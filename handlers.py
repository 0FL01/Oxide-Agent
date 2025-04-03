from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
from config import chat_history, huggingface_client, azure_client, together_client, groq_client, openrouter_client, mistral_client, MODELS, encode_image, process_file, DEFAULT_MODEL, gemini_client, TOGETHER_API_KEY
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

    text = update.message.text or update.message.caption or ""
    image = update.message.photo[-1] if update.message.photo else None
    document = update.message.document

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
        context.user_data['editing_prompt'] = False  # Сбрасываем флаг редактирования
        await update.message.reply_text(
            'Выберите действие: (Или начните диалог)',
            reply_markup=get_main_keyboard()
        )
    elif text in MODELS and not context.user_data.get('editing_prompt'):  # Добавлена проверка флага
        context.user_data['model'] = text
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    elif document:
        # Process single document
        file = await document.get_file()
        file_extension = os.path.splitext(document.file_name)[1].lower()
        file_path = f"temp_file_{update.effective_user.id}_{document.file_name}"

        try:
            await file.download_to_drive(file_path)
            file_content = process_file(file_path)
            full_message = f"\nСодержимое файла {document.file_name}:\n{file_content}\n"
            if text:
                full_message += f"\nЗапрос пользователя: {text}"
            await process_message(update, context, full_message)
        finally:
            if os.path.exists(file_path):
                os.remove(file_path)
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
    user_text = update.message.caption or ""

    # List of supported file extensions
    supported_extensions = [
        '.txt', '.log', '.xml', '.md',
        '.doc', '.docx', '.csv', '.xls', '.xlsx'
    ]

    if file_extension in supported_extensions:
        await update.message.reply_text("Обрабатываю файл, пожалуйста подождите...")
        file_path = f"temp_file_{user_id}{file_extension}"

        try:
            # Download and process the file
            await file.download_to_drive(file_path)
            file_content = process_file(file_path)

            # Create context message with file content
            context_message = (
                f"Содержимое файла {document.file_name}:\n\n"
                f"{file_content}\n\n"
                f"Запрос пользователя: {user_text}"
            )

            # Process the combined message
            await process_message(update, context, context_message)

        except Exception as e:
            error_msg = f"Произошла ошибка при обработке файла: {str(e)}"
            logger.error(f"Error processing file for user {user_id}: {str(e)}")
            await update.message.reply_text(error_msg)

        finally:
            # Clean up temporary file
            if os.path.exists(file_path):
                os.remove(file_path)
                logger.info(f"Temporary file {file_path} removed")
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
    chat_history = get_chat_history(user_id)

    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    logger.info(f"Selected model for user {user_id}: {selected_model}")

    if MODELS[selected_model].get("type") == "image":
        await generate_and_send_image(update, context, text)
        return

    image_path = None  # Инициализируем переменную для пути к изображению

    if image:
        # Если модель не поддерживает обработку изображений, отправляем сообщение пользователю
        if not MODELS[selected_model].get("vision", False):
            await update.message.reply_text("Выбранная модель не поддерживает обработку изображений.")
            return  # Прекращаем дальнейшую обработку, так как модель не может обработать изображение

        # Если модель поддерживает обработку изображений, продолжаем как обычно
        file = await image.get_file()
        image_path = f"temp_image_{user_id}.jpg"  # Сохраняем путь к изображению
        await file.download_to_drive(image_path)  # Сохраняем изображение на диск
        image_base64 = encode_image(image_path)  # Кодируем изображение в base64

        # Здесь можно добавить логику для обработки изображения, если модель поддерживает vision
        image_description = "Описание изображения будет здесь, если модель поддерживает vision."
        logger.info(f"User {user_id} ({user_name}) sent an image. Description: {image_description[:100]}...")

    full_message = text  # Используем только текст, так как изображение не обрабатывается
    
    logger.info(f"User {user_id} ({user_name}) sent: {full_message}")
    
    # Сохраняем сообщение пользователя
    save_message(user_id, "user", full_message)

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING)

        # Используем пользовательский промпт вместо стандартного
        messages = [{"role": "system", "content": system_message}] + get_chat_history(user_id)

        if MODELS[selected_model]["provider"] == "groq":
            response = await groq_client.chat.completions.create(
                messages=messages,
                model=MODELS[selected_model]["id"],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "mistral":
            if mistral_client is None:
                raise ValueError("Mistral client is not initialized. Please check your MISTRAL_API_KEY.")
            response = mistral_client.chat.complete(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": system_message}] + messages,
                temperature=0.9,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "huggingface":
            if huggingface_client is None:
                raise ValueError("Huggingface client is not initialized. Please check your HF_API_KEY.")
            response = huggingface_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": system_message}] + messages,
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "gemini":
            if gemini_client is None:
                raise ValueError("Gemini client is not initialized. Please check your GEMINI_API_KEY.")
            
            model = gemini_client.GenerativeModel(MODELS[selected_model]["id"])
            
            if image:
                try:
                    # Create proper Gemini content parts
                    contents = []
                    
                    # Add text part if provided
                    if text:
                        contents.append(text)
                    
                    # Add image part in Gemini's expected format
                    image_data = Image.open(image_path)
                    contents.append({
                        "mime_type": "image/jpeg",
                        "data": base64.b64encode(image_data.tobytes()).decode('utf-8')
                    })
                    
                    # Generate response
                    response = await asyncio.to_thread(
                        model.generate_content,
                        {
                            "contents": [{
                                "parts": contents
                            }]
                        },
                        generation_config=gemini_client.types.GenerationConfig(
                            max_output_tokens=MODELS[selected_model]["max_tokens"],
                            temperature=0.7,
                        )
                    )
                    
                    if not response.text:
                        raise ValueError("Gemini returned empty response")
                        
                except Exception as e:
                    logger.error(f"Error generating Gemini response: {str(e)}", exc_info=True)
                    raise ValueError(f"Ошибка при обработке изображения: {str(e)}")
                finally:
                    # Clean up temporary image file
                    if image_path and os.path.exists(image_path):
                        os.remove(image_path)
            else:
                # Обработка только текстового сообщения
                converted_messages = []
                for message in messages:
                    converted_messages.append({
                        "role": "user" if message["role"] == "user" else "model",
                        "parts": [message["content"]]
                    })
                
                response = await asyncio.to_thread(
                    model.generate_content,
                    converted_messages,
                    generation_config=gemini_client.types.GenerationConfig(
                        max_output_tokens=MODELS[selected_model]["max_tokens"],
                        temperature=0.7,
                    )
                )
            
            bot_response = response.text

        elif MODELS[selected_model]["provider"] == "together":
            if together_client is None:
                raise ValueError("Together AI client is not initialized. Please check your TOGETHER_API_KEY.")
            response = together_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": system_message}] + messages,
                temperature=0.8,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "openrouter":
            if openrouter_client is None:
                raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")
            response = openrouter_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": system_message}] + messages,
                temperature=0.8,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            if response.choices and len(response.choices) > 0 and response.choices[0].message:
                bot_response = response.choices[0].message.content
            else:
                raise ValueError("Опять API провайдер откис, воскреснет когда нибудь наверное")


        elif MODELS[selected_model]["provider"] == "azure":
            if azure_client is None:
                raise ValueError("Azure client is not initialized. Please check your GITHUB_TOKEN.")

            messages = [{"role": "system", "content": system_message}] + messages

            if image:
                # Обработка изображения для vision модели
                image_data_url = f"data:image/jpeg;base64,{image_base64}"
                messages.append({
                    "role": "user",
                    "content": [
                        {"type": "text", "text": text},
                        {"type": "image_url", "image_url": {"url": image_data_url, "detail": "low"}}
                    ]
                })
            else:
                messages.append({"role": "user", "content": text})

            response = azure_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=messages,
                temperature=0.8,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        else:
            raise ValueError(f"Unknown provider for model {selected_model}")

        # Сохраняем ответ ассистента
        save_message(user_id, "assistant", bot_response)
        logger.info(f"Sent response to user {user_id} ({user_name}): {bot_response}")

        formatted_response = format_text(bot_response)
        message_parts = split_long_message(formatted_response)

        for part in message_parts:
            try:
                await update.message.reply_text(part, parse_mode=ParseMode.HTML)
            except BadRequest as e:
                logger.error(f"Error sending message: {str(e)}")
                # Если возникла ошибка при отправке с HTML-разметкой, отправляем без разметки
                await update.message.reply_text(html.unescape(part), parse_mode=None)

    except Exception as e:
        logger.error(f"Error processing request for user {user_id}: {str(e)}")
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса: <code>{str(e)}</code>", parse_mode=ParseMode.HTML)


async def improve_prompt(prompt: str, gemini_client) -> str:
    if gemini_client is None:
        raise ValueError("Gemini client is not initialized. Please check your GEMINI_API_KEY.")
    
    model = gemini_client.GenerativeModel('gemini-2.0-flash')
    
    messages = [
        {
            "role": "user",
            "parts": [PROMPT_IMPROVEMENT_SYSTEM_MESSAGE, prompt]
        }
    ]

    response = model.generate_content(
        messages,
        generation_config=gemini_client.types.GenerationConfig(
            max_output_tokens=500,
            temperature=1,
        )
    )

    improved_prompt = response.text
    return improved_prompt

async def generate_and_send_image(update: Update, context: ContextTypes.DEFAULT_TYPE, prompt: str):
    user_id = update.effective_user.id
    try:
        await update.message.chat.send_action(action=ChatAction.UPLOAD_PHOTO)

        # Улучшение промпта с помощью Gemini
        improved_prompt = await improve_prompt(prompt, gemini_client)
        
        logger.info(f"Original prompt: {prompt}")
        logger.info(f"Improved prompt: {improved_prompt}")

        image_base64 = generate_image(improved_prompt)
        image_data = base64.b64decode(image_base64)

        with open(f"temp_image_{user_id}.png", "wb") as f:
            f.write(image_data)

        with open(f"temp_image_{user_id}.png", "rb") as f:
            await update.message.reply_photo(photo=f, caption=f"Сгенерировано изображение по улучшенному запросу: {improved_prompt}")

        os.remove(f"temp_image_{user_id}.png")

    except Exception as e:
        logger.error(f"error generating image for user {user_id}: {str(e)}")
        await update.message.reply_text(f"произошла ошибка при генерации изображения: {str(e)}")

def generate_image(prompt):
    if not TOGETHER_API_KEY:
        raise ValueError("TOGETHER_API_KEY is not set in the environment variables.")

    together_client = Together(api_key=TOGETHER_API_KEY)
    response = together_client.images.generate(
        prompt=prompt,
        model="black-forest-labs/FLUX.1-schnell-Free",
        width=1024,
        height=768,
        steps=1,
        n=1,
        response_format="b64_json"
    )
    return response.data[0].b64_json

ADMIN_ID = int(os.getenv('ADMIN_ID'))

def encode_image(image_path):
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode('utf-8')

def process_file(file_path: str, max_size: int = 1 * 1024 * 1024) -> str:
    if os.path.getsize(file_path) > max_size:
        raise ValueError(f"Файл слишком большой. Максимальный размер: {max_size/1024/1024}MB")

@check_auth
async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Received voice message from user {user_id}")
    temp_filename = f"tempvoice{user_id}.ogg"

    try:
        voice = await update.message.voice.get_file()
        voice_file = await voice.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(voice_file)
        with open(temp_filename, "rb") as audio_file:
            transcription = await groq_client.audio.transcriptions.create(
                file=(temp_filename, audio_file.read()),
                model="whisper-large-v3",
                language="ru"
            )

        recognized_text = transcription.text
        logger.info(f"Voice message from user {user_id} ({user_name}) recognized: {recognized_text}")

        await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}")
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Temporary file {temp_filename} removed")


@check_auth
@admin_required
async def add_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    try:
        new_user_id = int(context.args[0])
        role = UserRole(context.args[1].upper())
        add_allowed_user(new_user_id, role)
        await update.message.reply_text(f"Пользователь {new_user_id} успешно добавлен с ролью {role.value}.")
    except (ValueError, IndexError):
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER).")

@check_auth
@admin_required
async def remove_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    try:
        remove_user_id = int(context.args[0])
        remove_allowed_user(remove_user_id)
        await update.message.reply_text(f"Пользователь {remove_user_id} успешно удален.")
    except (ValueError, IndexError):
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя.")

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
    temp_filename = f"tempvideo_{user_id}.mp4"
    
    try:
        video = await update.message.video.get_file()
        video_bytes = await video.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(video_bytes)
        
        with open(temp_filename, "rb") as video_file:
            transcription = await groq_client.audio.transcriptions.create(
                file=(temp_filename, video_file.read()),
                model="whisper-large-v3",
                language="ru"
            )
        
        recognized_text = transcription.text
        logger.info(f"Видео сообщение от пользователя {user_id} ({user_name}) распознано: {recognized_text}")
        await process_message(update, context, recognized_text)
    
    except Exception as e:
        logger.error(f"Ошибка при обработке видео сообщения для пользователя {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке видео сообщения: {str(e)}")
    
    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Временный файл {temp_filename} удалён")



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

            # Паттерны для API ключей (примеры, адаптируйте под свои нужды)
            (r'(sk-[a-zA-Z0-9]{48})', '[OPENAI_API_KEY_MASKED]'), # OpenAI
            (r'(AIza[0-9A-Za-z\\-_]{35})', '[GEMINI_API_KEY_MASKED]'), # Google AI/Gemini
            (r'([a-zA-Z0-9]{32})', '[GENERIC_API_KEY_MASKED_32]'), # Общий 32-значный ключ
            (r'(ghp_[a-zA-Z0-9]{36})', '[GITHUB_TOKEN_MASKED]'), # GitHub Token

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
        # Преобразуем сообщение лога в строку
        log_message = record.getMessage()
        # Применяем все паттерны для маскирования
        for pattern, repl in self.compiled_patterns:
            log_message = pattern.sub(repl, log_message)
        # Обновляем сообщение записи лога
        record.msg = log_message
        # Очищаем кешированные данные, чтобы getMessage() использовал измененное msg
        record.args = () # Очищаем args, так как форматирование уже применено к msg
        record._message = None # Сбрасываем кешированное сообщение
        return True

def setup_logging():
    # Получаем корневой логгер
    root_logger = logging.getLogger()
    # Устанавливаем уровень логирования INFO (DEBUG, INFO, WARNING, ERROR, CRITICAL)
    root_logger.setLevel(logging.INFO)

    # Удаляем все существующие обработчики, чтобы избежать дублирования
    for handler in root_logger.handlers[:]:
        root_logger.removeHandler(handler)

    # Создаем обработчик для вывода в STDOUT
    stdout_handler = logging.StreamHandler(sys.stdout)
    # Устанавливаем уровень для обработчика (можно сделать отличным от логгера)
    stdout_handler.setLevel(logging.INFO)

    # Создаем форматтер для логов
    log_formatter = logging.Formatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(filename)s:%(lineno)d - %(message)s',
        datefmt='%Y-%m-%d %H:%M:%S'
    )
    # Применяем форматтер к обработчику
    stdout_handler.setFormatter(log_formatter)

    # Создаем и добавляем фильтр для маскирования данных
    sensitive_filter = SensitiveDataFilter()
    stdout_handler.addFilter(sensitive_filter)

    # Добавляем обработчик к корневому логгеру
    root_logger.addHandler(stdout_handler)

    # Конфигурируем логгер библиотеки telegram.ext, чтобы он тоже выводил в STDOUT
    # Можно установить другой уровень, если нужно меньше/больше логов от самой библиотеки
    telegram_logger = logging.getLogger("telegram.ext")
    telegram_logger.setLevel(logging.INFO) # Или WARNING, если не нужны INFO от библиотеки
    # Убедимся, что у него нет своих обработчиков и он использует корневой
    telegram_logger.handlers = []
    telegram_logger.propagate = True # Передавать сообщения корневому логгеру

    # Отключаем логгеры httpcore и httpx, чтобы избежать спама
    logging.getLogger("httpcore").setLevel(logging.WARNING)
    logging.getLogger("httpx").setLevel(logging.WARNING)

# Вызываем функцию настройки логирования при старте
setup_logging()

# Получаем логгер для текущего модуля ПОСЛЕ настройки
logger = logging.getLogger(__name__)
