# --- START OF FILE handlers.py ---

from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
# Добавляем импорт genai из config, если он там инициализируется
from config import chat_history, huggingface_client, azure_client, together_client, groq_client, openrouter_client, mistral_client, MODELS, process_file, DEFAULT_MODEL, gemini_client, TOGETHER_API_KEY
from PIL import Image
from utils import split_long_message, clean_html, format_text
from database import UserRole, is_user_allowed, add_allowed_user, remove_allowed_user, get_user_role, clear_chat_history, get_chat_history, save_message, update_user_prompt, get_user_prompt, get_user_model, update_user_model
from telegram.error import BadRequest
import html
import logging
import os
import re
import base64
import asyncio
from together import Together
from dotenv import load_dotenv
import sys
# Добавляем импорт для работы с типами Gemini
import google.generativeai as genai_types # Используем псевдоним, чтобы избежать конфликта имен

load_dotenv()

user_auth_states = {}

def set_user_auth_state(user_id: int, state: bool):
    user_auth_states[user_id] = state

DEFAULT_SYSTEM_MESSAGE = """Ты - полезный ассистент с искусственным интеллектом. Ты всегда стараешься дать точные и полезные ответы. Ты можешь общаться на разных языках, включая русский и английский."""

DEFAULT_PROMPT_IMPROVEMENT_MESSAGE = """Ты - эксперт по улучшению промптов для генерации изображений. Твоя задача - сделать промпт более детальным и эффективным, сохраняя при этом основную идею. Анализируй контекст и добавляй художественные детали."""

PROMPT_IMPROVEMENT_SYSTEM_MESSAGE = os.getenv('PROMPT_IMPROVEMENT_SYSTEM_MESSAGE', DEFAULT_PROMPT_IMPROVEMENT_MESSAGE)
SYSTEM_MESSAGE = os.getenv('SYSTEM_MESSAGE', DEFAULT_SYSTEM_MESSAGE)

logger = logging.getLogger(__name__)

def get_main_keyboard():
    keyboard = [
        [KeyboardButton("Очистить контекст"), KeyboardButton("Сменить модель")],
        [KeyboardButton("Доп функции")]
    ]
    return ReplyKeyboardMarkup(keyboard, resize_keyboard=True)

def get_extra_functions_keyboard():
    keyboard = [
        [KeyboardButton("Изменить промпт"), KeyboardButton("Назад")]
    ]
    return ReplyKeyboardMarkup(keyboard, resize_keyboard=True)

def get_model_keyboard():
    keyboard = [[KeyboardButton(model_name)] for model_name in MODELS.keys()]
    keyboard.append([KeyboardButton("Назад")])
    return ReplyKeyboardMarkup(keyboard, resize_keyboard=True)

def check_auth(func):
    async def wrapper(update: Update, context: ContextTypes.DEFAULT_TYPE):
        user_id = update.effective_user.id
        if not is_user_allowed(user_id):
            set_user_auth_state(user_id, False)
            await update.message.reply_text("Вы не авторизованы. Пожалуйста, введите /start для авторизации.")
            return
        set_user_auth_state(user_id, True)
        return await func(update, context)
    return wrapper

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    logger.info(f"User {user_id} started the bot")

    if not is_user_allowed(user_id):
        await update.message.reply_text("Пожалуйста, введите код авторизации:")
        return

    # Получаем сохраненную модель пользователя или используем модель по умолчанию
    saved_model = get_user_model(user_id)
    context.user_data['model'] = saved_model if saved_model else DEFAULT_MODEL

    set_user_auth_state(user_id, True)
    await update.message.reply_text(
        f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>{context.user_data["model"]}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

def admin_required(func):
    async def wrapper(update: Update, context: ContextTypes.DEFAULT_TYPE):
        user_id = update.effective_user.id
        user_role = get_user_role(user_id)
        if user_role != UserRole.ADMIN:
            await update.message.reply_text("У вас нет прав для выполнения этой команды.")
            return
        return await func(update, context)
    return wrapper

@check_auth
async def clear(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    try:
        clear_chat_history(user_id)
        logger.info(f"Chat history cleared for user {user_id}")
        await update.message.reply_text('<b>История чата очищена.</b>', parse_mode=ParseMode.HTML, reply_markup=get_main_keyboard())
    except Exception as e:
        logger.error(f"Error clearing chat history for user {user_id}: {e}")
        await update.message.reply_text('Произошла ошибка при очистке истории чата.')

@check_auth
async def change_model(update: Update, context: ContextTypes.DEFAULT_TYPE):
    text = update.message.text

    if text == "Сменить модель":
        # Показываем клавиатуру с моделями
        await update.message.reply_text(
            'Выберите модель:',
            reply_markup=get_model_keyboard()
        )
    elif text in MODELS:
        # Обновляем модель в памяти и базе данных
        context.user_data['model'] = text
        update_user_model(update.effective_user.id, text)
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )

@check_auth
async def handle_message(update: Update, context: ContextTypes.DEFAULT_TYPE):
    if not update.message:
        return

    text = update.message.text or update.message.caption or ""

    # Обработка режима редактирования промпта
    if context.user_data.get('editing_prompt'):
        if text == "Назад":
            context.user_data['editing_prompt'] = False
            await update.message.reply_text("Отмена обновления системного промпта.", reply_markup=get_main_keyboard())
        else:
            try:
                update_user_prompt(update.effective_user.id, text)
                context.user_data['editing_prompt'] = False
                await update.message.reply_text("Системный промпт обновлен.", reply_markup=get_main_keyboard())
            except Exception as e:
                logger.error(f"Ошибка обновления системного промпта для пользователя {update.effective_user.id}: {e}", exc_info=True)
                await update.message.reply_text("Произошла ошибка при обновлении системного промпта.", reply_markup=get_main_keyboard())
        return

    # Получаем текст, изображение или документ
    text = update.message.text or update.message.caption or ""
    image = update.message.photo[-1] if update.message.photo else None # Берем фото наибольшего разрешения
    document = update.message.document

    # Обработка команд и кнопок
    if text == "Очистить контекст":
        await clear(update, context)
    elif text == "Сменить модель":
        await change_model(update, context)
    elif text == "Доп функции":
        await update.message.reply_text("Выберите действие:", reply_markup=get_extra_functions_keyboard())
    elif text == "Изменить промпт":
        context.user_data['editing_prompt'] = True
        await update.message.reply_text("Введите новый системный промпт. Для отмены введите 'Назад':", reply_markup=get_extra_functions_keyboard())
    elif text == "Назад":
        context.user_data['editing_prompt'] = False
        await update.message.reply_text(
            'Выберите действие: (Или начните диалог)',
            reply_markup=get_main_keyboard()
        )
    elif text in MODELS and not context.user_data.get('editing_prompt'):
        context.user_data['model'] = text
        update_user_model(update.effective_user.id, text) # Сохраняем выбор модели
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    # Обработка документа (если не было других команд)
    elif document:
        await process_document(update, context, document) # Используем отдельную функцию для документов
    # Обработка текстового сообщения или сообщения с изображением
    else:
        await process_message(update, context, text, image)


async def process_document(update: Update, context: ContextTypes.DEFAULT_TYPE, document):
    """
    Process incoming document files from Telegram users.
    Supports multiple file formats and integrates content into chat context.
    """
    user_id = update.effective_user.id
    file = await document.get_file()
    file_extension = os.path.splitext(document.file_name)[1].lower()
    user_text = update.message.caption or "" # Текст, прикрепленный к документу

    # List of supported file extensions (из utils.py)
    supported_extensions = [
        '.txt', '.log', '.md', '.xml', '.docx', '.doc', '.xlsx', '.xls', '.csv'
    ]

    if file_extension in supported_extensions:
        await update.message.chat.send_action(action=ChatAction.UPLOAD_DOCUMENT) # Показываем статус обработки
        file_path = f"temp_doc_{user_id}_{document.file_name}" # Уникальное имя файла

        try:
            # Download and process the file
            await file.download_to_drive(file_path)
            # Используем process_file из utils.py, предполагая, что он импортирован
            file_content = process_file(file_path)

            # Create context message with file content and user query
            context_message = (
                f"Содержимое файла {document.file_name}:\n\n"
                f"{file_content}\n\n"
            )
            # Добавляем запрос пользователя, если он есть
            if user_text:
                 context_message += f"Запрос пользователя: {user_text}"
            else:
                 context_message += "Проанализируй содержимое файла." # Запрос по умолчанию

            # Process the combined message (файл + опциональный текст)
            # Передаем None как image, так как это обработка документа
            await process_message(update, context, context_message, image=None)

        except ValueError as ve: # Ловим ошибки размера файла или типа из process_file
             logger.warning(f"File processing error for user {user_id}: {str(ve)}")
             await update.message.reply_text(str(ve))
        except Exception as e:
            error_msg = f"Произошла ошибка при обработке файла: {str(e)}"
            logger.error(f"Error processing file for user {user_id}: {str(e)}", exc_info=True)
            await update.message.reply_text(error_msg)
        finally:
            # Clean up temporary file
            if os.path.exists(file_path):
                try:
                    os.remove(file_path)
                    logger.info(f"Temporary file {file_path} removed")
                except OSError as oe:
                     logger.error(f"Error removing temporary file {file_path}: {oe}")
    else:
        supported_formats = ", ".join(supported_extensions)
        await update.message.reply_text(
            f"Неподдерживаемый тип файла: {file_extension}\n"
            f"Поддерживаемые форматы: {supported_formats}"
        )


async def process_message(update: Update, context: ContextTypes.DEFAULT_TYPE, text: str, image=None):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name

    # Получаем пользовательский промпт или используем стандартный
    user_prompt = get_user_prompt(user_id)
    system_message = user_prompt if user_prompt else SYSTEM_MESSAGE

    # Получаем историю чата из базы данных
    chat_history_db = get_chat_history(user_id) # Переименовано во избежание конфликта

    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    logger.info(f"Selected model for user {user_id}: {selected_model}")

    # --- Проверка поддержки изображений МОДЕЛЬЮ ---
    model_info = MODELS.get(selected_model, {})
    model_supports_vision = model_info.get("vision", False)
    model_provider = model_info.get("provider")
    model_is_image_generator = model_info.get("type") == "image"

    # Если модель - генератор изображений
    if model_is_image_generator:
        # Игнорируем присланное изображение, используем только текст как промпт
        await generate_and_send_image(update, context, text)
        return

    # Если пришло изображение, но модель его не поддерживает
    if image and not model_supports_vision:
        await update.message.reply_text(f"Модель '{selected_model}' не поддерживает обработку изображений. Вы можете сменить модель или отправить только текст.")
        return # Прекращаем обработку

    image_path = None # Путь к временному файлу изображения
    image_bytes = None # Байты изображения для Gemini
    image_base64 = None # Base64 для Azure

    try:
        # --- Обработка изображения (если есть и модель поддерживает) ---
        if image and model_supports_vision:
            await update.message.chat.send_action(action=ChatAction.UPLOAD_PHOTO) # Показываем статус
            file = await image.get_file()
            image_path = f"temp_image_{user_id}_{file.file_unique_id}.jpg" # Уникальное имя
            await file.download_to_drive(image_path)
            logger.info(f"Image downloaded for user {user_id} to {image_path}")

            # Подготовка данных изображения для нужного провайдера
            if model_provider == "gemini":
                try:
                    # Читаем байты файла для Gemini
                    with open(image_path, "rb") as img_file:
                        image_bytes = img_file.read()
                    # MIME тип можно определить точнее, но для Telegram часто jpg
                    # image_part = genai_types.types.Part.from_bytes(data=image_bytes, mime_type="image/jpeg")
                    # logger.info(f"Image prepared as Gemini Part for user {user_id}")
                    # Оставляем image_bytes, Part создадим ниже
                except Exception as e:
                    logger.error(f"Error reading image file for Gemini: {e}", exc_info=True)
                    await update.message.reply_text("Ошибка при чтении файла изображения.")
                    if os.path.exists(image_path): os.remove(image_path)
                    return
            elif model_provider == "azure":
                 # Кодируем в Base64 для Azure
                 image_base64 = encode_image(image_path)
                 logger.info(f"Image encoded to base64 for Azure for user {user_id}")
            # Добавить другие провайдеры с vision при необходимости

        # --- Подготовка сообщения и истории ---
        # Сохраняем сообщение пользователя (текст)
        # Если было изображение, текст может быть пустым (caption)
        user_message_content = text if text else "[Изображение без текста]"
        save_message(user_id, "user", user_message_content) # Сохраняем только текст запроса

        await update.message.chat.send_action(action=ChatAction.TYPING)

        # Формируем историю сообщений для API
        messages_for_api = [{"role": "system", "content": system_message}] + chat_history_db

        # --- Вызов API конкретного провайдера ---

        bot_response = None # Инициализируем переменную для ответа

        if model_provider == "groq":
            # Groq не поддерживает изображения в текущей реализации
            messages_for_api.append({"role": "user", "content": text})
            response = await groq_client.chat.completions.create(
                messages=messages_for_api,
                model=model_info["id"],
                temperature=0.7,
                max_tokens=model_info["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif model_provider == "mistral":
             # Mistral API Client (официальный) может поддерживать vision в будущем,
             # но текущая реализация в коде - текстовая
            messages_for_api.append({"role": "user", "content": text})
            if mistral_client is None:
                raise ValueError("Mistral client is not initialized.")
            response = mistral_client.chat.complete(
                model=model_info["id"],
                messages=messages_for_api,
                temperature=0.9,
                max_tokens=model_info["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif model_provider == "huggingface":
             # HF Inference API - обычно текстовый, vision зависит от конкретной модели
            messages_for_api.append({"role": "user", "content": text})
            if huggingface_client is None:
                raise ValueError("Huggingface client is not initialized.")
            response = huggingface_client.chat.completions.create(
                model=model_info["id"],
                messages=messages_for_api,
                temperature=0.7,
                max_tokens=model_info["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif model_provider == "gemini":
            if gemini_client is None:
                raise ValueError("Gemini client is not initialized.")

            model = gemini_client.GenerativeModel(model_info["id"])
            generation_config = genai_types.types.GenerationConfig(
                 max_output_tokens=model_info["max_tokens"],
                 temperature=0.7,
            )

            if image and image_bytes: # Если есть изображение (и байты прочитаны)
                # Формируем контент согласно документации Gemini Vision
                gemini_contents = []
                if text:
                    gemini_contents.append(text) # Текст идет первым

                # Добавляем изображение как Part
                # MIME тип можно определить точнее, если нужно (например, библиотекой mimetypes)
                # Пока предполагаем jpeg, т.к. Telegram часто конвертирует в него
                image_part = genai_types.types.Part.from_bytes(data=image_bytes, mime_type="image/jpeg")
                gemini_contents.append(image_part)

                logger.info(f"Sending to Gemini with image. Content parts: {len(gemini_contents)}")
                # Используем простую структуру для одного запроса с картинкой
                response = await asyncio.to_thread(
                    model.generate_content,
                    contents=gemini_contents, # Передаем список [текст, картинка] или [картинка]
                    generation_config=generation_config
                )
                # Структура {"contents": [{"parts": ...}]} больше подходит для мульти-тёрн чата

            else: # Только текст
                # Преобразуем историю в формат Gemini
                converted_messages = []
                # Добавляем системный промпт как первый user message, если модель его так воспринимает
                # Или обрабатываем его отдельно, если API позволяет
                # Текущая реализация добавляет его в `messages_for_api` выше
                # Нужно проверить, как Gemini лучше обрабатывает system prompt в generate_content

                # Преобразуем историю чата
                for msg in chat_history_db: # Используем только историю из БД
                     role = "user" if msg["role"] == "user" else "model"
                     converted_messages.append({"role": role, "parts": [msg["content"]]})
                # Добавляем текущее сообщение пользователя
                converted_messages.append({"role": "user", "parts": [text]})

                logger.info(f"Sending text-only message to Gemini. History length: {len(converted_messages)}")
                response = await asyncio.to_thread(
                    model.generate_content,
                    converted_messages, # Передаем историю сообщений
                    generation_config=generation_config
                )

            if not response.text:
                 # Проверяем наличие ошибки в ответе
                 try:
                     error_info = response.prompt_feedback
                     logger.error(f"Gemini API error: {error_info}")
                     raise ValueError(f"Gemini API Error: {error_info}")
                 except (AttributeError, ValueError) as e:
                     logger.error(f"Gemini returned empty response and no specific error info.")
                     raise ValueError("Gemini вернул пустой ответ.") from e

            bot_response = response.text

        elif model_provider == "together":
             # Together API Client - текстовый в текущей реализации
            messages_for_api.append({"role": "user", "content": text})
            if together_client is None:
                raise ValueError("Together AI client is not initialized.")
            response = together_client.chat.completions.create(
                model=model_info["id"],
                messages=messages_for_api,
                temperature=0.8,
                max_tokens=model_info["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif model_provider == "openrouter":
             # OpenRouter - текстовый в текущей реализации
            messages_for_api.append({"role": "user", "content": text})
            if openrouter_client is None:
                raise ValueError("OpenRouter client is not initialized.")
            response = openrouter_client.chat.completions.create(
                model=model_info["id"],
                messages=messages_for_api,
                temperature=0.8,
                max_tokens=model_info["max_tokens"],
            )
            if response.choices and len(response.choices) > 0 and response.choices[0].message:
                bot_response = response.choices[0].message.content
            else:
                # Логируем детали ответа, если есть
                logger.error(f"OpenRouter API returned unexpected response: {response}")
                raise ValueError("API провайдер (OpenRouter) вернул некорректный ответ.")


        elif model_provider == "azure":
            # Azure OpenAI (включая GPT-4o Vision)
            if azure_client is None:
                raise ValueError("Azure client is not initialized.")

            # Формируем сообщения для Azure API
            azure_messages = [{"role": "system", "content": system_message}] + chat_history_db # Берем историю из БД

            if image and image_base64: # Если есть изображение и base64 для Azure
                # Формируем контент с текстом и изображением
                message_content = [{"type": "text", "text": text if text else "Опиши это изображение."}]
                image_url_part = {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/jpeg;base64,{image_base64}", "detail": "low"} # Используем low detail для экономии
                }
                message_content.append(image_url_part)
                azure_messages.append({"role": "user", "content": message_content})
                logger.info(f"Sending to Azure with image. Message content parts: {len(message_content)}")
            else: # Только текст
                azure_messages.append({"role": "user", "content": text})
                logger.info("Sending text-only message to Azure.")

            response = azure_client.chat.completions.create(
                model=model_info["id"],
                messages=azure_messages,
                temperature=0.8,
                max_tokens=model_info["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        else:
            raise ValueError(f"Unknown or unsupported provider for model {selected_model}: {model_provider}")

        # --- Отправка ответа пользователю ---
        if bot_response:
            # Сохраняем ответ ассистента
            save_message(user_id, "assistant", bot_response)
            logger.info(f"Sent response to user {user_id} ({user_name}): {bot_response[:100]}...") # Логируем начало ответа

            formatted_response = format_text(bot_response)
            message_parts = split_long_message(formatted_response)

            for part in message_parts:
                try:
                    await update.message.reply_text(part, parse_mode=ParseMode.HTML)
                except BadRequest as e:
                    logger.warning(f"Error sending message part with HTML: {str(e)}. Sending as plain text.")
                    # Пытаемся отправить без разметки, удаляя HTML-сущности
                    plain_text_part = html.unescape(re.sub('<[^<]+?>', '', part)) # Простая очистка от тегов
                    try:
                        await update.message.reply_text(plain_text_part, parse_mode=None)
                    except Exception as final_e:
                         logger.error(f"Failed to send message part even as plain text: {final_e}")
                         # Можно отправить общее сообщение об ошибке, если и это не удалось
                         if part == message_parts[0]: # Только для первой части, чтобы не спамить
                             await update.message.reply_text("Не удалось отправить отформатированный ответ.")

        else:
             logger.warning(f"Bot response was empty for user {user_id}")
             # Не отправляем пустое сообщение, но логируем

    except Exception as e:
        logger.error(f"Error processing request for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса.\n<code>{html.escape(str(e))}</code>", parse_mode=ParseMode.HTML)
    finally:
        # --- Очистка временного файла изображения ---
        if image_path and os.path.exists(image_path):
            try:
                os.remove(image_path)
                logger.info(f"Temporary image file {image_path} removed")
            except OSError as oe:
                logger.error(f"Error removing temporary image file {image_path}: {oe}")


async def improve_prompt(prompt: str, gemini_client_instance) -> str: # Передаем инстанс клиента
    if gemini_client_instance is None:
        raise ValueError("Gemini client is not initialized. Cannot improve prompt.")

    # Убедимся, что используем правильный клиент genai
    model = gemini_client_instance.GenerativeModel('gemini-1.5-flash') # Используем доступную модель для улучшения

    # Формируем сообщение для улучшения промпта
    messages = [
        {"role": "user", "parts": [PROMPT_IMPROVEMENT_SYSTEM_MESSAGE, prompt]}
    ]

    try:
        response = await asyncio.to_thread(
             model.generate_content,
             messages,
             generation_config=genai_types.types.GenerationConfig( # Используем псевдоним
                 max_output_tokens=500, # Ограничение на длину улучшенного промпта
                 temperature=0.8, # Немного креативности
             )
        )
        improved_prompt = response.text
        if not improved_prompt:
             logger.warning("Prompt improvement returned empty text. Using original prompt.")
             return prompt # Возвращаем оригинал, если улучшение не удалось
        return improved_prompt
    except Exception as e:
        logger.error(f"Error improving prompt: {e}", exc_info=True)
        return prompt # Возвращаем оригинал в случае ошибки

async def generate_and_send_image(update: Update, context: ContextTypes.DEFAULT_TYPE, prompt: str):
    user_id = update.effective_user.id
    try:
        await update.message.chat.send_action(action=ChatAction.TYPING) # Сначала думаем над промптом

        # Улучшение промпта с помощью Gemini (передаем клиент)
        improved_prompt = await improve_prompt(prompt, gemini_client)

        logger.info(f"Original prompt: {prompt}")
        logger.info(f"Improved prompt: {improved_prompt}")

        await update.message.reply_text(f"Улучшенный промпт:\n<code>{html.escape(improved_prompt)}</code>\n\nГенерирую изображение...", parse_mode=ParseMode.HTML)
        await update.message.chat.send_action(action=ChatAction.UPLOAD_PHOTO) # Теперь генерируем

        image_base64 = generate_image(improved_prompt) # Используем улучшенный промпт
        image_data = base64.b64decode(image_base64)

        temp_image_path = f"temp_generated_image_{user_id}.png"
        with open(temp_image_path, "wb") as f:
            f.write(image_data)

        with open(temp_image_path, "rb") as f:
            # Отправляем фото с улучшенным промптом в caption
            await update.message.reply_photo(photo=f, caption=f"Изображение по запросу: {improved_prompt}")

        os.remove(temp_image_path)

    except Exception as e:
        logger.error(f"Error generating image for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при генерации изображения: {str(e)}")

def generate_image(prompt):
    if not TOGETHER_API_KEY:
        raise ValueError("TOGETHER_API_KEY is not set in the environment variables.")

    # Убедимся, что клиент Together инициализирован (он должен быть в config.py)
    if together_client is None:
         raise ValueError("Together AI client is not initialized.")

    try:
        response = together_client.images.generate(
            prompt=prompt,
            model="stabilityai/stable-diffusion-xl-1024-v1.0", # Пример модели, можно заменить на black-forest-labs/FLUX.1-schnell-Free если доступна
            width=1024,
            height=1024, # Используем квадратный формат для SDXL
            # steps=1, # steps=1 может быть слишком мало для SDXL, используем значение по умолчанию или больше
            n=1,
            response_format="b64_json"
        )
        if not response.data or not response.data[0].b64_json:
             raise ValueError("API генерации изображений вернуло пустой результат.")
        return response.data[0].b64_json
    except Exception as e:
         logger.error(f"Error calling Together Image API: {e}")
         raise # Перебрасываем исключение дальше

ADMIN_ID = int(os.getenv('ADMIN_ID'))

def encode_image(image_path):
    """Кодирует изображение в Base64 строку."""
    try:
        with open(image_path, "rb") as image_file:
            return base64.b64encode(image_file.read()).decode('utf-8')
    except Exception as e:
        logger.error(f"Error encoding image {image_path} to base64: {e}")
        raise

# Функция process_file остается в utils.py, здесь ее дублировать не нужно
# def process_file(file_path: str, max_size: int = 1 * 1024 * 1024) -> str:
#     ...

@check_auth
async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Received voice message from user {user_id}")
    temp_filename = f"tempvoice_{user_id}.ogg" # Уникальное имя

    try:
        await update.message.chat.send_action(action=ChatAction.RECORD_VOICE) # Показываем статус
        voice = await update.message.voice.get_file()
        voice_file_bytes = await voice.download_as_bytearray() # Скачиваем в память

        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем статус распознавания

        # Используем BytesIO для передачи данных в Groq без сохранения на диск
        from io import BytesIO
        audio_file_like = BytesIO(voice_file_bytes)
        audio_file_like.name = "audio.ogg" # Groq требует имя файла

        if groq_client is None:
             raise ValueError("Groq client is not initialized.")

        transcription = await groq_client.audio.transcriptions.create(
            file=(audio_file_like.name, audio_file_like), # Передаем имя и файлоподобный объект
            model="whisper-large-v3",
            language="ru" # Указываем язык для лучшего распознавания
        )

        recognized_text = transcription.text
        logger.info(f"Voice message from user {user_id} ({user_name}) recognized: {recognized_text}")

        # Передаем распознанный текст в основной обработчик сообщений
        await process_message(update, context, recognized_text, image=None) # Передаем None как image

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    # finally: # Временный файл больше не используется
    #     if os.path.exists(temp_filename):
    #         os.remove(temp_filename)
    #         logger.info(f"Temporary file {temp_filename} removed")


@check_auth
@admin_required
async def add_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    if not context.args or len(context.args) != 2:
        await update.message.reply_text("Использование: /add_user <telegram_id> <ROLE>\nРоли: ADMIN, USER")
        return
    try:
        new_user_id = int(context.args[0])
        role_str = context.args[1].upper()
        if role_str not in UserRole.__members__:
             await update.message.reply_text(f"Неверная роль '{context.args[1]}'. Доступные роли: ADMIN, USER")
             return
        role = UserRole(role_str)
        add_allowed_user(new_user_id, role)
        await update.message.reply_text(f"Пользователь {new_user_id} успешно добавлен с ролью {role.value}.")
        logger.info(f"Admin {update.effective_user.id} added user {new_user_id} with role {role.value}")
    except ValueError:
        await update.message.reply_text("Пожалуйста, укажите корректный числовой ID пользователя.")
    except Exception as e:
         logger.error(f"Error in add_user command: {e}", exc_info=True)
         await update.message.reply_text("Произошла ошибка при добавлении пользователя.")


@check_auth
@admin_required
async def remove_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    if not context.args or len(context.args) != 1:
        await update.message.reply_text("Использование: /remove_user <telegram_id>")
        return
    try:
        remove_user_id = int(context.args[0])
        remove_allowed_user(remove_user_id)
        await update.message.reply_text(f"Пользователь {remove_user_id} успешно удален.")
        logger.info(f"Admin {update.effective_user.id} removed user {remove_user_id}")
    except ValueError:
        await update.message.reply_text("Пожалуйста, укажите корректный числовой ID пользователя.")
    except Exception as e:
         logger.error(f"Error in remove_user command: {e}", exc_info=True)
         await update.message.reply_text("Произошла ошибка при удалении пользователя.")

async def healthcheck(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """
    Обработчик команды /healthcheck. Возвращает "OK" если бот работает.
    """
    await update.message.reply_text("OK")

@check_auth
async def handle_video(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Получено видео сообщение от пользователя {user_id}")
    temp_filename = f"tempvideo_{user_id}.mp4" # Уникальное имя

    try:
        await update.message.chat.send_action(action=ChatAction.RECORD_VIDEO) # Показываем статус
        video = await update.message.video.get_file()
        video_bytes = await video.download_as_bytearray() # Скачиваем в память

        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем статус распознавания

        # Используем BytesIO для передачи данных в Groq без сохранения на диск
        from io import BytesIO
        video_file_like = BytesIO(video_bytes)
        video_file_like.name = "video.mp4" # Groq требует имя файла

        if groq_client is None:
             raise ValueError("Groq client is not initialized.")

        transcription = await groq_client.audio.transcriptions.create(
            file=(video_file_like.name, video_file_like), # Передаем имя и файлоподобный объект
            model="whisper-large-v3", # Whisper может обрабатывать аудио из видео
            language="ru" # Указываем язык
        )

        recognized_text = transcription.text
        logger.info(f"Видео сообщение от пользователя {user_id} ({user_name}) распознано: {recognized_text}")

        # Передаем распознанный текст в основной обработчик сообщений
        await process_message(update, context, recognized_text, image=None) # Передаем None как image

    except Exception as e:
        logger.error(f"Ошибка при обработке видео сообщения для пользователя {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке видео сообщения: {str(e)}")
    # finally: # Временный файл больше не используется
    #     if os.path.exists(temp_filename):
    #         os.remove(temp_filename)
    #         logger.info(f"Временный файл {temp_filename} удалён")



class SensitiveDataFilter(logging.Filter):
    def __init__(self):
        super().__init__()
        # Паттерны для маскирования чувствительных данных
        # Можно добавить больше паттернов по мере необходимости
        self.patterns = [
            # Существующие паттерны для токена Telegram
            (r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)', r'\1[TELEGRAM_TOKEN_MASKED]\3'),
            (r'([0-9]{8,10}:[A-Za-z0-9_-]{35})', '[TELEGRAM_TOKEN_MASKED]'),
            (r'(bot[0-9]{8,10}:)[A-Za-z0-9_-]+', r'\1[TELEGRAM_TOKEN_MASKED]'),

            (r'(AIza[0-9A-Za-z\\-_]{35})', '[GEMINI_API_KEY_MASKED]'), # Google AI/Gemini
            (r'(ghp_[a-zA-Z0-9]{36})', '[GITHUB_TOKEN_MASKED]'), # GitHub Token
            (r'(gsk_[a-zA-Z0-9]{48})', '[GROQ_API_KEY_MASKED]'), # Groq API Key (пример)
            (r'(ts_[a-zA-Z0-9]{60,})', '[TOGETHER_API_KEY_MASKED]'), # Together API Key (пример)

            # Паттерны для данных БД (если используются и могут попасть в логи)
            (r"'user': '[^']*'", "'user': '[DB_USER_MASKED]'"),
            (r"'password': '[^']*'", "'password': '[DB_PASSWORD_MASKED]'"),
            (r"'dbname': '[^']*'", "'dbname': '[DB_NAME_MASKED]'"),
            (r"'host': '[^']*'", "'host': '[DB_HOST_MASKED]'"),
            (r"'port': '[^']*'", "'port': '[DB_PORT_MASKED]'"),
        ]
        # Компилируем регулярные выражения для производительности
        self.compiled_patterns = [(re.compile(pattern), repl) for pattern, repl in self.patterns]

    def filter(self, record):
        # Преобразуем сообщение лога в строку, если это еще не сделано
        if isinstance(record.msg, (dict, list, tuple)):
             log_message = str(record.msg)
        else:
             log_message = record.getMessage() # Получаем отформатированное сообщение

        original_message = log_message # Сохраняем для сравнения

        # Применяем все паттерны для маскирования
        for pattern, repl in self.compiled_patterns:
            log_message = pattern.sub(repl, log_message)

        # Обновляем сообщение записи лога, только если оно изменилось
        if log_message != original_message:
            record.msg = log_message
            # Очищаем кешированные данные, чтобы getMessage() использовал измененное msg
            record.args = () # Очищаем args, так как форматирование уже применено к msg
            record._message = log_message # Принудительно обновляем кешированное сообщение

        return True

def setup_logging():
    root_logger = logging.getLogger()
    root_logger.setLevel(logging.INFO)

    for handler in root_logger.handlers[:]:
        root_logger.removeHandler(handler)

    stdout_handler = logging.StreamHandler(sys.stdout)
    stdout_handler.setLevel(logging.INFO)

    log_formatter = logging.Formatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(filename)s:%(lineno)d - %(message)s',
        datefmt='%Y-%m-%d %H:%M:%S'
    )
    stdout_handler.setFormatter(log_formatter)

    sensitive_filter = SensitiveDataFilter()
    stdout_handler.addFilter(sensitive_filter)

    root_logger.addHandler(stdout_handler)

    telegram_logger = logging.getLogger("telegram.ext")
    telegram_logger.setLevel(logging.INFO) 
    telegram_logger.handlers = []
    telegram_logger.propagate = True 

    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("httpx").setLevel(logging.WARNING)

setup_logging()

logger = logging.getLogger(__name__)