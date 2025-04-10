from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
import google.api_core.exceptions
from config import chat_history, groq_client, mistral_client, MODELS, DEFAULT_MODEL, gemini_client
from utils import split_long_message, clean_html, format_text
from database import UserRole, is_user_allowed, add_allowed_user, remove_allowed_user, get_user_role, clear_chat_history, get_chat_history, save_message, update_user_prompt, get_user_prompt, get_user_model, update_user_model
from telegram.error import BadRequest
import html
import logging
import os
import re
import asyncio
from dotenv import load_dotenv
import google.generativeai as genai

load_dotenv()

user_auth_states = {}

def set_user_auth_state(user_id: int, state: bool):
    user_auth_states[user_id] = state

DEFAULT_SYSTEM_MESSAGE = """Ты - полезный ассистент с искусственным интеллектом. Ты всегда стараешься дать точные и полезные ответы. Ты можешь общаться на разных языках, включая русский и английский."""

SYSTEM_MESSAGE = os.getenv('SYSTEM_MESSAGE', DEFAULT_SYSTEM_MESSAGE)

logger = logging.getLogger(__name__)

# --- Функция для транскрипции через Gemini ---
MAX_RETRIES = 3
RETRY_DELAY_SECONDS = 3

async def audio_to_text(file_path: str, mime_type: str) -> str:
    if not gemini_client:
        raise Exception("Клиент Google Gemini не инициализирован (проверьте GEMINI_API_KEY).")

    transcription_model_name = "Gemini 2.0 Flash"
    if transcription_model_name not in MODELS or MODELS[transcription_model_name]['provider'] != 'gemini':
         raise Exception(f"Модель '{transcription_model_name}' не найдена или не является моделью Gemini в конфигурации.")

    model_id = MODELS[transcription_model_name]['id']
    gemini_model_instance = gemini_client.GenerativeModel(model_id)
    logger.info(f"Используется модель Gemini '{model_id}' для транскрипции.")

    uploaded_file = None
    last_exception = None

    try:
        for attempt in range(MAX_RETRIES):
            try:
                logger.info(f"Попытка {attempt + 1}/{MAX_RETRIES} загрузки файла {file_path} (MIME: {mime_type}) в Google File API...")
                uploaded_file = await asyncio.to_thread(
                    lambda: genai.upload_file(path=file_path, mime_type=mime_type)
                )
                if not uploaded_file or not hasattr(uploaded_file, 'name') or not hasattr(uploaded_file, 'uri'):
                     logger.error(f"Не удалось получить корректный объект файла после загрузки {file_path} на попытке {attempt + 1}.")
                     # Не бросаем исключение сразу, даем шанс следующей попытке, если это не последняя
                     if attempt == MAX_RETRIES - 1:
                         raise Exception("Ошибка Google File API: Не удалось загрузить файл или получить его данные после нескольких попыток.")
                     last_exception = Exception("Ошибка Google File API: Некорректный ответ при загрузке файла.") # Сохраняем для последней попытки
                     await asyncio.sleep(RETRY_DELAY_SECONDS * (attempt + 1)) # Задержка перед следующей попыткой
                     continue # Переходим к следующей попытке

                logger.info(f"Файл успешно загружен на попытке {attempt + 1}: {uploaded_file.name}, URI: {uploaded_file.uri}")
                last_exception = None # Сбрасываем ошибку при успехе
                break # Выходим из цикла при успешной загрузке

            except google.api_core.exceptions.ServiceUnavailable as e:
                last_exception = e
                logger.warning(f"Попытка загрузки {attempt + 1} не удалась (503 Service Unavailable): {e}. Повтор через {RETRY_DELAY_SECONDS * (attempt + 1)} сек...")
                if attempt == MAX_RETRIES - 1:
                    logger.error(f"Загрузка файла {file_path} не удалась после {MAX_RETRIES} попыток.")
                    raise # Пробрасываем исключение после последней попытки
                await asyncio.sleep(RETRY_DELAY_SECONDS * (attempt + 1))
            except Exception as e:
                 logger.error(f"Неперехватываемая ошибка при загрузке файла {file_path} на попытке {attempt + 1}: {e}", exc_info=True)
                 raise # Пробрасываем другие ошибки немедленно

        if last_exception: # Если цикл завершился из-за ошибок, но исключение не было проброшено (например, некорректный объект файла)
            logger.error(f"Загрузка файла {file_path} окончательно не удалась.")
            raise last_exception

        await asyncio.sleep(2)

        response = None
        last_exception = None # Сбрасываем для следующего цикла

        for attempt in range(MAX_RETRIES):
            try:
                logger.info(f"Попытка {attempt + 1}/{MAX_RETRIES} запроса транскрипции для файла {uploaded_file.name} через Gemini API...")
                prompt = "Сделай точную транскрипцию речи из этого аудио/видео файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это."
                response = await gemini_model_instance.generate_content_async(
                    contents=[prompt, uploaded_file],
                    generation_config=genai.types.GenerationConfig(
                        temperature=0.4
                    ),
                    safety_settings={'HARASSMENT':'block_none',
                                     'HATE_SPEECH':'block_none',
                                     'SEXUALLY_EXPLICIT':'block_none',}
                )
                logger.info(f"Получен ответ от Gemini API для транскрипции файла {uploaded_file.name} на попытке {attempt + 1}.")
                last_exception = None # Сбрасываем ошибку при успехе
                break # Выходим из цикла при успешном получении ответа

            except google.api_core.exceptions.ServiceUnavailable as e:
                last_exception = e
                logger.warning(f"Попытка транскрипции {attempt + 1} не удалась (503 Service Unavailable): {e}. Повтор через {RETRY_DELAY_SECONDS * (attempt + 1)} сек...")
                if attempt == MAX_RETRIES - 1:
                    logger.error(f"Транскрипция файла {uploaded_file.name} не удалась после {MAX_RETRIES} попыток.")
                    raise # Пробрасываем исключение после последней попытки
                await asyncio.sleep(RETRY_DELAY_SECONDS * (attempt + 1))
            except Exception as e:
                 logger.error(f"Неперехватываемая ошибка при транскрипции файла {uploaded_file.name} на попытке {attempt + 1}: {e}", exc_info=True)
                 raise # Пробрасываем другие ошибки немедленно

        if last_exception: # Если цикл завершился из-за ошибок
             logger.error(f"Получение транскрипции для {uploaded_file.name} окончательно не удалось.")
             raise last_exception

        if response and hasattr(response, 'text') and response.text:
            transcript_text = response.text.strip()
            logger.info(f"Транскрипция для {uploaded_file.name} получена (длина: {len(transcript_text)}).")
            if "не могу обработать" in transcript_text.lower() or "не содержит речи" in transcript_text.lower() or "не удалось извлечь аудио" in transcript_text.lower():
                 logger.warning(f"Gemini вернул сообщение о невозможности транскрипции для {uploaded_file.name}: {transcript_text}")
                 return f"(Gemini): {transcript_text}"
            return transcript_text
        elif response and hasattr(response, 'prompt_feedback') and response.prompt_feedback and hasattr(response.prompt_feedback, 'block_reason') and response.prompt_feedback.block_reason:
             block_reason = response.prompt_feedback.block_reason
             block_reason_message = response.prompt_feedback.block_reason_message if hasattr(response.prompt_feedback, 'block_reason_message') else 'Нет деталей'
             logger.warning(f"Транскрипция заблокирована Gemini по причине: {block_reason}. Детали: {block_reason_message}")
             raise Exception(f"Ошибка Gemini API: Транскрипция заблокирована (причина: {block_reason}).")
        elif response and hasattr(response, 'candidates') and response.candidates:
             try:
                 candidate_text = response.candidates[0].content.parts[0].text
                 if candidate_text:
                     logger.warning(f"Основной текст ответа Gemini пуст, но найден текст в response.candidates для {uploaded_file.name}. Используется он.")
                     return candidate_text.strip()
                 else:
                     raise AttributeError("Текст в candidates пуст")
             except (AttributeError, IndexError, TypeError) as e:
                 logger.warning(f"Не удалось извлечь текст транскрипции из response.candidates для {uploaded_file.name}. Ошибка: {e}. Ответ: {response}")
                 raise Exception("Ошибка Gemini API: Не удалось получить транскрипцию (пустой или некорректный ответ).")
        else:
             logger.warning(f"Gemini API вернул пустой или неожиданный ответ для транскрипции файла {uploaded_file.name}: {response}")
             raise Exception("Ошибка Gemini API: Не удалось получить транскрипцию (пустой или неожиданный ответ).")

    except Exception as e:
        logger.error(f"Ошибка при транскрипции файла {file_path} через Gemini: {e}", exc_info=True)
        error_str = str(e)
        if isinstance(e, google.api_core.exceptions.ServiceUnavailable):
             # Если ошибка 503 проброшена после всех ретраев
             raise Exception(f"Ошибка Gemini API: Сервис недоступен после {MAX_RETRIES} попыток (503).")
        elif "API key not valid" in error_str:
            raise Exception("Ошибка Gemini API: Неверный или неактивный ключ API.")
        elif "quota" in error_str.lower() or "resource_exhausted" in error_str.lower():
            raise Exception("Ошибка Gemini API: Превышена квота использования.")
        elif "File processing failed" in error_str or "Unable to process the file" in error_str:
             raise Exception("Ошибка Gemini API: Не удалось обработать загруженный файл (возможно, неподдерживаемый формат или поврежден).")
        elif "Deadline exceeded" in error_str or "504" in error_str:
             raise Exception("Ошибка Gemini API: Превышено время ожидания ответа от сервера.")
        raise Exception(f"Ошибка Gemini API при транскрипции: {error_str}")

    finally:
        if uploaded_file and hasattr(uploaded_file, 'name'):
            try:
                logger.info(f"Удаление файла {uploaded_file.name} из Google File API...")
                await asyncio.to_thread(lambda: genai.delete_file(name=uploaded_file.name))
                logger.info(f"Файл {uploaded_file.name} успешно удален.")
            except Exception as delete_e:
                logger.error(f"Не удалось удалить файл {uploaded_file.name} из Google File API: {delete_e}")


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
        logger.info(f"Checking auth for user {user_id} for function {func.__name__}")
        if not is_user_allowed(user_id):
            set_user_auth_state(user_id, False)
            logger.warning(f"User {user_id} is not authorized.")
            await update.message.reply_text("Вы не авторизованы. Пожалуйста, введите /start для авторизации.")
            return
        set_user_auth_state(user_id, True)
        logger.info(f"User {user_id} is authorized.")
        return await func(update, context)
    return wrapper

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"User {user_id} ({user_name}) initiated /start command.")

    if not is_user_allowed(user_id):
        logger.info(f"User {user_id} is not allowed, requesting authorization code.")
        await update.message.reply_text("Пожалуйста, введите код авторизации:")
        return

    saved_model = get_user_model(user_id)
    context.user_data['model'] = saved_model if saved_model else DEFAULT_MODEL
    logger.info(f"User {user_id} is allowed. Set model to {context.user_data['model']}.")

    set_user_auth_state(user_id, True)
    reply_text = f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>{context.user_data["model"]}</b>'
    logger.info(f"Sending welcome message to user {user_id}.")
    await update.message.reply_text(
        reply_text,
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

def admin_required(func):
    async def wrapper(update: Update, context: ContextTypes.DEFAULT_TYPE):
        user_id = update.effective_user.id
        user_name = update.effective_user.username or update.effective_user.first_name
        logger.info(f"Checking admin privileges for user {user_id} ({user_name}) for function {func.__name__}.")
        user_role = get_user_role(user_id)
        if user_role != UserRole.ADMIN:
            logger.warning(f"User {user_id} ({user_name}) does not have admin privileges for {func.__name__}.")
            await update.message.reply_text("У вас нет прав для выполнения этой команды.")
            return
        logger.info(f"User {user_id} ({user_name}) has admin privileges.")
        return await func(update, context)
    return wrapper

@check_auth
async def clear(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"User {user_id} ({user_name}) initiated context clear.")
    try:
        clear_chat_history(user_id)
        logger.info(f"Chat history successfully cleared for user {user_id}.")
        await update.message.reply_text('<b>История чата очищена.</b>', parse_mode=ParseMode.HTML, reply_markup=get_main_keyboard())
    except Exception as e:
        logger.error(f"Error clearing chat history for user {user_id}: {e}")
        await update.message.reply_text('Произошла ошибка при очистке истории чата.')

@check_auth
async def change_model(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    text = update.message.text
    logger.info(f"User {user_id} ({user_name}) initiated model change with text: '{text}'.")

    if text == "Сменить модель":
        logger.info(f"Showing model selection keyboard to user {user_id}.")
        await update.message.reply_text(
            'Выберите модель:',
            reply_markup=get_model_keyboard()
        )
    elif text in MODELS:
        context.user_data['model'] = text
        update_user_model(user_id, text)
        logger.info(f"Model changed to '{text}' for user {user_id}.")
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    else:
         logger.warning(f"User {user_id} sent unexpected text '{text}' during model change.")
         await handle_message(update, context)

@check_auth
async def handle_message(update: Update, context: ContextTypes.DEFAULT_TYPE):
    if not update.message:
        logger.warning("Received an update without a message.")
        return

    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    text = update.message.text or update.message.caption or ""
    document = update.message.document
    logger.info(f"Handling message from user {user_id} ({user_name}). Text: '{text[:100]}...'. Document attached: {bool(document)}")

    if document:
        logger.warning(f"User {user_id} sent an unsupported document: {document.file_name}")
        await update.message.reply_text("Данный файл не поддерживается.")
        return

    if context.user_data.get('editing_prompt'):
        logger.info(f"User {user_id} is in prompt editing mode.")
        if text == "Назад":
            context.user_data['editing_prompt'] = False
            logger.info(f"User {user_id} cancelled prompt editing.")
            await update.message.reply_text("Отмена обновления системного промпта.", reply_markup=get_main_keyboard())
        else:
            try:
                update_user_prompt(user_id, text)
                context.user_data['editing_prompt'] = False
                logger.info(f"System prompt updated for user {user_id}.")
                await update.message.reply_text("Системный промпт обновлен.", reply_markup=get_main_keyboard())
            except Exception as e:
                logger.error(f"Error updating system prompt for user {user_id}: {e}", exc_info=True)
                await update.message.reply_text("Произошла ошибка при обновлении системного промпта.", reply_markup=get_main_keyboard())
        return

    if text == "Очистить контекст":
        logger.info(f"User {user_id} clicked 'Очистить контекст'.")
        await clear(update, context)
    elif text == "Сменить модель":
        logger.info(f"User {user_id} clicked 'Сменить модель'.")
        await change_model(update, context)
    elif text == "Доп функции":
        logger.info(f"User {user_id} clicked 'Доп функции'.")
        await update.message.reply_text("Выберите действие:", reply_markup=get_extra_functions_keyboard())
    elif text == "Изменить промпт":
        context.user_data['editing_prompt'] = True
        logger.info(f"User {user_id} clicked 'Изменить промпт', entering editing mode.")
        await update.message.reply_text("Введите новый системный промпт. Для отмены введите 'Назад':", reply_markup=get_extra_functions_keyboard())
    elif text == "Назад":
        context.user_data['editing_prompt'] = False
        logger.info(f"User {user_id} clicked 'Назад' from extra functions menu.")
        await update.message.reply_text(
            'Выберите действие: (Или начните диалог)',
            reply_markup=get_main_keyboard()
        )
    elif text in MODELS and not context.user_data.get('editing_prompt'):
        logger.info(f"User {user_id} selected model '{text}' via text input.")
        context.user_data['model'] = text
        update_user_model(user_id, text)
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    else:
        logger.info(f"Processing regular text message from user {user_id}.")
        await process_message(update, context, text)

async def process_message(update: Update, context: ContextTypes.DEFAULT_TYPE, text: str):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Starting message processing for user {user_id} ({user_name}). Message snippet: '{text[:100]}...'")

    user_prompt = get_user_prompt(user_id)
    system_message = user_prompt if user_prompt else SYSTEM_MESSAGE
    logger.info(f"Using system message for user {user_id}: '{system_message[:100]}...'")

    chat_history_db = get_chat_history(user_id)
    logger.info(f"Retrieved {len(chat_history_db)} messages from history for user {user_id}.")

    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    logger.info(f"Selected model for user {user_id}: {selected_model} (Provider: {MODELS[selected_model]['provider']})")

    full_message = text
    logger.info(f"Saving user message for user {user_id} ({user_name}): '{full_message[:100]}...'")
    save_message(user_id, "user", full_message)

    try:
        logger.info(f"Sending typing action to chat {update.message.chat.id} for user {user_id}.")
        await update.message.chat.send_action(action=ChatAction.TYPING)

        messages = [{"role": "system", "content": system_message}] + chat_history_db + [{"role": "user", "content": full_message}]
        logger.info(f"Prepared {len(messages)} messages for API call for user {user_id}.")

        bot_response = ""
        provider = MODELS[selected_model]["provider"]
        model_id = MODELS[selected_model]["id"]
        max_tokens = MODELS[selected_model]["max_tokens"]
        logger.info(f"Making API call to {provider} with model {model_id} for user {user_id}.")

        if provider == "groq":
            if groq_client is None:
                 raise ValueError("Groq client is not initialized.")
            response = await groq_client.chat.completions.create(
                messages=messages,
                model=model_id,
                temperature=0.7,
                max_tokens=max_tokens,
            )
            bot_response = response.choices[0].message.content
            logger.info(f"Received response from Groq for user {user_id}.")

        elif provider == "mistral":
            if mistral_client is None:
                raise ValueError("Mistral client is not initialized. Please check your MISTRAL_API_KEY.")
            response = mistral_client.chat.complete(
                model=model_id,
                messages=messages,
                temperature=0.9,
                max_tokens=max_tokens,
            )
            bot_response = response.choices[0].message.content
            logger.info(f"Received response from Mistral for user {user_id}.")

        elif provider == "gemini":
            if gemini_client is None:
                raise ValueError("Gemini client is not initialized. Please check your GEMINI_API_KEY.")

            model = gemini_client.GenerativeModel(
                model_id,
                system_instruction=system_message
            )
            # Конвертируем историю для Gemini API
            converted_messages = []
            for message in chat_history_db:
                 # Пропускаем системное сообщение, так как оно передается отдельно
                 if message["role"] != "system":
                     converted_messages.append({
                         "role": "user" if message["role"] == "user" else "model",
                         "parts": [message["content"]]
                     })
            # Добавляем текущее сообщение пользователя
            converted_messages.append({"role": "user", "parts": [full_message]})

            logger.info(f"Sending {len(converted_messages)} converted messages (plus system prompt) to Gemini for user {user_id}.")
            response = await model.generate_content_async( # Используем async версию
                converted_messages,
                generation_config=gemini_client.types.GenerationConfig(
                    max_output_tokens=max_tokens,
                    temperature=1,
                ),
                safety_settings={'HARASSMENT':'block_none',
                                 'HATE_SPEECH':'block_none',
                                 'SEXUALLY_EXPLICIT':'block_none'}
            )
            # Проверка на блокировку ответа
            if not response.text and hasattr(response, 'prompt_feedback') and response.prompt_feedback.block_reason:
                block_reason = response.prompt_feedback.block_reason
                logger.warning(f"Ответ Gemini заблокирован по причине: {block_reason}")
                bot_response = f"_(Ответ заблокирован Gemini по причине: {block_reason})_"
            else:
                bot_response = response.text

            logger.info(f"Received response from Gemini for user {user_id}.")

        else:
            logger.error(f"Unknown provider '{provider}' for model {selected_model} requested by user {user_id}.")
            raise ValueError(f"Unknown provider for model {selected_model}")

        logger.info(f"Saving assistant response for user {user_id}. Snippet: '{bot_response[:100]}...'")
        save_message(user_id, "assistant", bot_response)

        logger.info(f"Formatting response for Telegram for user {user_id}.")
        formatted_response = format_text(bot_response)
        logger.info(f"Splitting response into chunks if necessary for user {user_id}.")
        message_parts = split_long_message(formatted_response)
        logger.info(f"Sending response in {len(message_parts)} part(s) to user {user_id}.")

        for i, part in enumerate(message_parts):
            try:
                logger.info(f"Sending part {i+1}/{len(message_parts)} to user {user_id}.")
                await update.message.reply_text(part, parse_mode=ParseMode.HTML)
            except BadRequest as e:
                logger.error(f"BadRequest error sending part {i+1} to user {user_id}: {str(e)}. Sending raw text.")
                # Попытка отправить без форматирования
                raw_part = re.sub('<[^<]+?>', '', part) # Удаляем HTML теги
                try:
                    await update.message.reply_text(raw_part, parse_mode=None)
                except Exception as inner_e:
                    logger.error(f"Failed to send raw text part {i+1} as well for user {user_id}: {inner_e}")
                    await update.message.reply_text(f"Ошибка при отправке части ответа (попытка 2): {str(inner_e)}")

            except Exception as e:
                 logger.error(f"Unexpected error sending part {i+1} to user {user_id}: {str(e)}", exc_info=True)
                 await update.message.reply_text(f"Ошибка при отправке части ответа: {str(e)}")

    except Exception as e:
        logger.error(f"Error processing message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса: <code>{html.escape(str(e))}</code>", parse_mode=ParseMode.HTML)

ADMIN_ID = int(os.getenv('ADMIN_ID'))

@check_auth
async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Received voice message from user {user_id} ({user_name}).")
    temp_filename = f"tempvoice_{user_id}.ogg"
    logger.info(f"Preparing to download voice to {temp_filename}.")

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем индикатор работы
        voice = await update.message.voice.get_file()
        voice_file = await voice.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(voice_file)
        logger.info(f"Voice message downloaded to {temp_filename}. Transcribing using Gemini...")

        # --- Замена Groq Whisper на Gemini ---
        recognized_text = await audio_to_text(temp_filename, 'audio/ogg')
        # --- Конец замены ---

        logger.info(f"Voice message from user {user_id} ({user_name}) transcribed by Gemini: '{recognized_text}'")

        # Проверяем, не вернула ли Gemini сообщение об ошибке
        if recognized_text.startswith("(Gemini):"):
            logger.warning(f"Gemini returned a notice for user {user_id}: {recognized_text}")
            await update.message.reply_text(f"Не удалось распознать речь: {recognized_text}")
        elif not recognized_text:
             logger.warning(f"Transcription result is empty for user {user_id}.")
             await update.message.reply_text("Не удалось распознать речь (пустой результат).")
        else:
            logger.info(f"Processing transcribed text for user {user_id}.")
            # Отправляем распознанный текст пользователю для подтверждения (опционально)
            await update.message.reply_text(f"Распознано: \"{recognized_text}\"\n\nОбрабатываю запрос...")
            await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    finally:
        if os.path.exists(temp_filename):
            try:
                os.remove(temp_filename)
                logger.info(f"Temporary voice file {temp_filename} removed for user {user_id}.")
            except Exception as e:
                logger.error(f"Error removing temporary file {temp_filename}: {e}")


@check_auth
@admin_required
async def add_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    admin_user_id = update.effective_user.id
    admin_user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Admin user {admin_user_id} ({admin_user_name}) initiated add_user command.")
    try:
        new_user_id = int(context.args[0])
        role_str = context.args[1].upper()
        role = UserRole(role_str)
        logger.info(f"Attempting to add user {new_user_id} with role {role.value} by admin {admin_user_id}.")
        add_allowed_user(new_user_id, role)
        logger.info(f"User {new_user_id} successfully added with role {role.value} by admin {admin_user_id}.")
        await update.message.reply_text(f"Пользователь {new_user_id} успешно добавлен с ролью {role.value}.")
    except (ValueError, IndexError):
        logger.warning(f"Admin {admin_user_id} provided invalid arguments for add_user: {context.args}")
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER). Пример: /add_user 123456789 USER")
    except Exception as e:
        logger.error(f"Error in add_user command initiated by admin {admin_user_id}: {e}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при добавлении пользователя: {str(e)}")

@check_auth
@admin_required
async def remove_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    admin_user_id = update.effective_user.id
    admin_user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Admin user {admin_user_id} ({admin_user_name}) initiated remove_user command.")
    try:
        remove_user_id = int(context.args[0])
        logger.info(f"Attempting to remove user {remove_user_id} by admin {admin_user_id}.")
        remove_allowed_user(remove_user_id)
        logger.info(f"User {remove_user_id} successfully removed by admin {admin_user_id}.")
        await update.message.reply_text(f"Пользователь {remove_user_id} успешно удален.")
    except (ValueError, IndexError):
        logger.warning(f"Admin {admin_user_id} provided invalid arguments for remove_user: {context.args}")
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя. Пример: /remove_user 123456789")
    except Exception as e:
        logger.error(f"Error in remove_user command initiated by admin {admin_user_id}: {e}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при удалении пользователя: {str(e)}")

async def healthcheck(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id if update.effective_user else "Unknown"
    logger.info(f"Healthcheck command received from user {user_id}.")
    await update.message.reply_text("OK")
    logger.info(f"Responded 'OK' to healthcheck from user {user_id}.")

@check_auth
async def handle_video(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Received video message from user {user_id} ({user_name}).")
    temp_filename = f"tempvideo_{user_id}.mp4"
    logger.info(f"Preparing to download video to {temp_filename}.")

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем индикатор работы
        video = await update.message.video.get_file()
        video_bytes = await video.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(video_bytes)
        logger.info(f"Video message downloaded to {temp_filename}. Transcribing audio track using Gemini...")

        # --- Замена Groq Whisper на Gemini ---
        # Указываем MIME-тип для видео
        recognized_text = await audio_to_text(temp_filename, 'video/mp4')
        # --- Конец замены ---

        logger.info(f"Video message from user {user_id} ({user_name}) transcribed by Gemini: '{recognized_text}'")

        # Проверяем, не вернула ли Gemini сообщение об ошибке
        if recognized_text.startswith("(Gemini):"):
            logger.warning(f"Gemini returned a notice for user {user_id}: {recognized_text}")
            await update.message.reply_text(f"Не удалось распознать речь из видео: {recognized_text}")
        elif not recognized_text:
             logger.warning(f"Video transcription result is empty for user {user_id}.")
             await update.message.reply_text("Не удалось распознать речь из видео (пустой результат).")
        else:
            logger.info(f"Processing transcribed text from video for user {user_id}.")
            # Отправляем распознанный текст пользователю для подтверждения (опционально)
            await update.message.reply_text(f"Распознано из видео: \"{recognized_text}\"\n\nОбрабатываю запрос...")
            await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing video message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке видео сообщения: {str(e)}")

    finally:
        if os.path.exists(temp_filename):
            try:
                os.remove(temp_filename)
                logger.info(f"Temporary video file {temp_filename} removed for user {user_id}.")
            except Exception as e:
                logger.error(f"Error removing temporary file {temp_filename}: {e}")

# Класс SensitiveDataFilter остается без изменений, так как он не затрагивается задачей
class SensitiveDataFilter(logging.Filter):
    def __init__(self):
        super().__init__()
        self.patterns = [
            (r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)', r'\1[TELEGRAM_TOKEN]\3'),
            (r'([0-9]{8,10}:[A-Za-z0-9_-]{35})', '[TELEGRAM_TOKEN]'),
            (r'(bot[0-9]{8,10}:)[A-Za-z0-9_-]+', r'\1[TELEGRAM_TOKEN]'),

            (r"'user': '[^']*'", "'user': '[MASKED]'"),
            (r"'password': '[^']*'", "'password': '[MASKED]'"),
            (r"'dbname': '[^']*'", "'dbname': '[MASKED]'"),
            (r"'host': '[^']*'", "'host': '[MASKED]'"),
            (r"'port': '[^']*'", "'port': '[MASKED]'")
        ]

    def filter(self, record):
        # Реализация фильтрации остается прежней
        if hasattr(record, 'msg'):
            if isinstance(record.msg, str):
                original_msg = record.msg
                for pattern, replacement in self.patterns:
                    record.msg = re.sub(pattern, replacement, record.msg)

        if hasattr(record, 'args'):
            if record.args:
                args_list = list(record.args)
                for i, arg in enumerate(args_list):
                    if isinstance(arg, str):
                        original_arg = arg
                        for pattern, replacement in self.patterns:
                            args_list[i] = re.sub(pattern, replacement, args_list[i])
                record.args = tuple(args_list)
        return True