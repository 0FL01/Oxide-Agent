from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
from google.genai import errors as genai_errors
    chat_history, groq_client, mistral_client, MODELS, DEFAULT_MODEL, gemini_client,
    openrouter_client, OPENROUTER_API_KEY, OPENROUTER_BASE_URL, OPENROUTER_SITE_URL, OPENROUTER_SITE_NAME,
    ADMIN_ID
)
from utils import split_long_message, clean_html, format_text
from database import UserRole, is_user_allowed, add_allowed_user, remove_allowed_user, get_user_role, clear_chat_history, get_chat_history, save_message, update_user_prompt, get_user_prompt, get_user_model, update_user_model, list_allowed_users, get_allowed_user
from telegram.error import BadRequest
import html
import logging
import os
import re
import asyncio
import base64
from dotenv import load_dotenv
from google.genai import types
from functools import wraps

load_dotenv()

user_auth_states = {}

def set_user_auth_state(user_id: int, state: bool):
    user_auth_states[user_id] = state

DEFAULT_SYSTEM_MESSAGE = """Ты - полезный ассистент с искусственным интеллектом. Ты всегда стараешься дать точные и полезные ответы. Ты можешь общаться на разных языках, включая русский и английский."""

SYSTEM_MESSAGE = os.getenv('SYSTEM_MESSAGE', DEFAULT_SYSTEM_MESSAGE)

logger = logging.getLogger(__name__)

# --- Декоратор с fallback на резервную модель ---
def retry_with_model_fallback():
    """Декоратор с fallback на резервную модель"""
    def decorator(func):
        @wraps(func)
        async def wrapper(*args, **kwargs):
            # Сначала пытаемся с gemini-flash-latest
            primary_model = 'gemini-flash-latest'
            fallback_model = 'gemini-2.5-flash'
            
            # 3 попытки с основной моделью
            for attempt in range(3):
                try:
                    logger.info(f"Attempting with {primary_model}, attempt {attempt + 1}/3")
                    return await func(*args, model=primary_model, **kwargs)
                except Exception as e:
                    error_str = str(e).lower()
                    logger.warning(f"Error with {primary_model} on attempt {attempt + 1}: {str(e)}")
                    
                    if attempt < 2:  # Не последняя попытка с основной моделью
                        await asyncio.sleep(2)
                        continue
                    else:
                        logger.info(f"All attempts with {primary_model} failed, switching to {fallback_model}")
                        break
            
            # Переключаемся на резервную модель с ретраями
            last_exception = None
            for attempt in range(5):
                try:
                    logger.info(f"Attempting with {fallback_model}, attempt {attempt + 1}/5")
                    return await func(*args, model=fallback_model, **kwargs)
                except Exception as e:
                    last_exception = e
                    error_str = str(e).lower()
                    
                    # Проверяем на временные ошибки API
                    if any(code in error_str for code in ['503', '429', '500', 'overloaded', 'unavailable', 'timeout']):
                        if attempt < 4:  # Не последняя попытка
                            logger.warning(f"API error with {fallback_model} on attempt {attempt + 1}/5: {str(e)}. Retrying in 3 seconds...")
                            await asyncio.sleep(3)
                            continue
                    
                    # Если это не временная ошибка или последняя попытка, поднимаем исключение
                    raise e
            
            # Если все попытки исчерпаны
            raise last_exception
        return wrapper
    return decorator

# --- Функция для транскрипции через Gemini с поддержкой fallback ---
@retry_with_model_fallback()
async def audio_to_text_with_model(file_path: str, mime_type: str, model: str = 'gemini-2.5-flash') -> str:
    """Функция транскрипции с поддержкой fallback между моделями"""
    if not gemini_client:
        raise Exception("Клиент Google Gemini не инициализирован (проверьте GEMINI_API_KEY).")

    logger.info(f"Используется модель Gemini '{model}' для транскрипции.")

    try:
        # Читаем аудиофайл как байты
        with open(file_path, 'rb') as f:
            audio_bytes = f.read()
        
        logger.info(f"Загружен аудиофайл {file_path} размером {len(audio_bytes)} байт. Отправляю на транскрипцию...")
        
        prompt = "Сделай точную транскрипцию речи из этого аудио/видео файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это."
        
        # Используем Part.from_bytes вместо загрузки через File API
        audio_part = types.Part.from_bytes(
            data=audio_bytes,
            mime_type=mime_type,
        )
        
        response = await asyncio.to_thread(
            lambda: gemini_client.models.generate_content(
                model=model,
                contents=[prompt, audio_part],
                config=types.GenerateContentConfig(
                    temperature=0.4,
                    safety_settings=[
                        types.SafetySetting(category='HARM_CATEGORY_HARASSMENT', threshold='BLOCK_NONE'),
                        types.SafetySetting(category='HARM_CATEGORY_HATE_SPEECH', threshold='BLOCK_NONE'),
                        types.SafetySetting(category='HARM_CATEGORY_SEXUALLY_EXPLICIT', threshold='BLOCK_NONE'),
                    ]
                )
            )
        )
        logger.info(f"Получен ответ от Gemini API для транскрипции файла {file_path} с моделью {model}.")

        if response and hasattr(response, 'text') and response.text:
            transcript_text = response.text.strip()
            logger.info(f"Транскрипция для {file_path} получена (длина: {len(transcript_text)}).")
            if "не могу обработать" in transcript_text.lower() or "не содержит речи" in transcript_text.lower() or "не удалось извлечь аудио" in transcript_text.lower():
                logger.warning(f"Gemini вернул сообщение о невозможности транскрипции для {file_path}: {transcript_text}")
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
                    logger.warning(f"Основной текст ответа Gemini пуст, но найден текст в response.candidates для {file_path}. Используется он.")
                    return candidate_text.strip()
                else:
                    raise AttributeError("Текст в candidates пуст")
            except (AttributeError, IndexError, TypeError) as e:
                logger.warning(f"Не удалось извлечь текст транскрипции из response.candidates для {file_path}. Ошибка: {e}. Ответ: {response}")
                raise Exception("Ошибка Gemini API: Не удалось получить транскрипцию (пустой или некорректный ответ).")
        else:
            logger.warning(f"Gemini API вернул пустой или неожиданный ответ для транскрипции файла {file_path}: {response}")
            raise Exception("Ошибка Gemini API: Не удалось получить транскрипцию (пустой или неожиданный ответ).")

    except Exception as e:
        logger.error(f"Ошибка при транскрипции файла {file_path} через Gemini: {e}", exc_info=True)
        error_str = str(e)
        if isinstance(e, genai_errors.APIError):
            # Если ошибка API - проверяем код ошибки
            if hasattr(e, 'code') and e.code == 503:
                raise Exception(f"Ошибка Gemini API: Сервис недоступен (503).")
            else:
                raise Exception(f"Ошибка Gemini API: {e}")
        elif "API key not valid" in error_str:
            raise Exception("Ошибка Gemini API: Неверный или неактивный ключ API.")
        elif "quota" in error_str.lower() or "resource_exhausted" in error_str.lower():
            raise Exception("Ошибка Gemini API: Превышена квота использования.")
        elif "File processing failed" in error_str or "Unable to process the file" in error_str:
            raise Exception("Ошибка Gemini API: Не удалось обработать загруженный файл (возможно, неподдерживаемый формат или поврежден).")
        elif "Deadline exceeded" in error_str or "504" in error_str:
            raise Exception("Ошибка Gemini API: Превышено время ожидания ответа от сервера.")
        raise Exception(f"Ошибка Gemini API при транскрипции: {error_str}")

# --- Функция для анализа изображений через Gemini с поддержкой fallback ---
@retry_with_model_fallback()
async def vision_analysis_with_model(photo_bytes: bytes, text_prompt: str, system_message: str, model: str = 'gemini-2.5-flash') -> str:
    """Функция анализа изображений с поддержкой fallback между моделями"""
    if not gemini_client:
        raise Exception("Клиент Google Gemini не инициализирован (проверьте GEMINI_API_KEY).")

    logger.info(f"Используется модель Gemini '{model}' для анализа изображения.")

    try:
        mime_type = "image/jpeg"  # Предполагаем JPEG для простоты
        
        # Формируем contents согласно документации
        contents = [
            types.Part.from_text(text=text_prompt),
            types.Part.from_bytes(
                data=photo_bytes,
                mime_type=mime_type,
            ),
        ]

        logger.info(f"Отправляю изображение и промпт на анализ в Gemini модель '{model}'.")

        response = await asyncio.to_thread(
            lambda: gemini_client.models.generate_content(
                model=model,
                contents=contents,
                config=types.GenerateContentConfig(
                    system_instruction=system_message,
                    max_output_tokens=4000,
                    temperature=0.7,
                    safety_settings=[
                        types.SafetySetting(category='HARM_CATEGORY_HARASSMENT', threshold='BLOCK_NONE'),
                        types.SafetySetting(category='HARM_CATEGORY_HATE_SPEECH', threshold='BLOCK_NONE'),
                        types.SafetySetting(category='HARM_CATEGORY_SEXUALLY_EXPLICIT', threshold='BLOCK_NONE'),
                    ]
                )
            )
        )

        # Проверка на блокировку ответа
        if not response.text and hasattr(response, 'prompt_feedback') and response.prompt_feedback and hasattr(response.prompt_feedback, 'block_reason') and response.prompt_feedback.block_reason:
            block_reason = response.prompt_feedback.block_reason
            block_reason_message = response.prompt_feedback.block_reason_message if hasattr(response.prompt_feedback, 'block_reason_message') else 'Нет деталей'
            logger.warning(f"Ответ Gemini заблокирован по причине: {block_reason}. Детали: {block_reason_message}")
            return f"_(Ответ заблокирован Gemini по причине: {block_reason})_"
        else:
            return response.text

    except Exception as e:
        logger.error(f"Ошибка при анализе изображения через Gemini: {e}", exc_info=True)
        error_str = str(e)
        if isinstance(e, genai_errors.APIError):
            if hasattr(e, 'code') and e.code == 503:
                raise Exception(f"Ошибка Gemini API: Сервис недоступен (503).")
            else:
                raise Exception(f"Ошибка Gemini API: {e}")
        elif "API key not valid" in error_str:
            raise Exception("Ошибка Gemini API: Неверный или неактивный ключ API.")
        elif "quota" in error_str.lower() or "resource_exhausted" in error_str.lower():
            raise Exception("Ошибка Gemini API: Превышена квота использования.")
        raise Exception(f"Ошибка Gemini API при анализе изображения: {error_str}")

# --- Функция для транскрипции через Gemini (обратная совместимость) ---
MAX_RETRIES = 3
RETRY_DELAY_SECONDS = 3

async def audio_to_text(file_path: str, mime_type: str) -> str:
    """Обертка для обратной совместимости - использует новую функцию с fallback"""
    try:
        return await audio_to_text_with_model(file_path, mime_type)
    except Exception as e:
        logger.error(f"Fallback transcription failed for {file_path}: {e}")
        raise e

# --- Функции для работы с OpenRouter API ---
# Модели для ретраев OpenRouter
OPENROUTER_PRIMARY_MODEL = "google/gemini-3-flash-preview"  # Основная модель
OPENROUTER_FALLBACK_MODEL = "google/gemini-2.5-flash"  # Резервная модель при недоступности основной

async def openrouter_chat_completion(
    messages: list,
    model: str,
    max_tokens: int = 64000,
    temperature: float = 0.7,
) -> str:
    """Отправляет запрос в OpenRouter API"""
    if not openrouter_client or not OPENROUTER_API_KEY:
        raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")
    
    headers = {
        "Authorization": f"Bearer {OPENROUTER_API_KEY}",
        "Content-Type": "application/json",
    }
    
    if OPENROUTER_SITE_URL:
        headers["HTTP-Referer"] = OPENROUTER_SITE_URL
    if OPENROUTER_SITE_NAME:
        headers["X-Title"] = OPENROUTER_SITE_NAME
    
    payload = {
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
    }
    
    logger.info(f"Sending request to OpenRouter with model {model}")
    
    response = await openrouter_client.post(
        OPENROUTER_BASE_URL,
        headers=headers,
        json=payload,
    )
    
    if response.status_code != 200:
        error_text = response.text
        logger.error(f"OpenRouter API error: {response.status_code} - {error_text}")
        raise Exception(f"OpenRouter API error: {response.status_code} - {error_text}")
    
    result = response.json()
    
    if "choices" not in result or not result["choices"]:
        logger.error(f"OpenRouter API returned empty choices: {result}")
        raise Exception("OpenRouter API returned empty response")
    
    return result["choices"][0]["message"]["content"]

async def openrouter_chat_with_retry(
    messages: list,
    max_tokens: int = 64000,
    temperature: float = 0.7,
) -> str:
    """Отправляет запрос в OpenRouter с поддержкой ретраев и fallback модели"""
    primary_model = OPENROUTER_PRIMARY_MODEL
    fallback_model = OPENROUTER_FALLBACK_MODEL
    
    # 3 попытки с основной моделью
    for attempt in range(3):
        try:
            logger.info(f"OpenRouter: Attempting with {primary_model}, attempt {attempt + 1}/3")
            return await openrouter_chat_completion(messages, primary_model, max_tokens, temperature)
        except Exception as e:
            error_str = str(e).lower()
            logger.warning(f"OpenRouter: Error with {primary_model} on attempt {attempt + 1}: {str(e)}")
            
            if attempt < 2:
                await asyncio.sleep(2)
                continue
            else:
                logger.info(f"OpenRouter: All attempts with {primary_model} failed, switching to {fallback_model}")
                break
    
    # Переключаемся на резервную модель с ретраями
    last_exception = None
    for attempt in range(5):
        try:
            logger.info(f"OpenRouter: Attempting with {fallback_model}, attempt {attempt + 1}/5")
            return await openrouter_chat_completion(messages, fallback_model, max_tokens, temperature)
        except Exception as e:
            last_exception = e
            error_str = str(e).lower()
            
            if any(code in error_str for code in ['503', '429', '500', 'overloaded', 'unavailable', 'timeout']):
                if attempt < 4:
                    logger.warning(f"OpenRouter: API error with {fallback_model} on attempt {attempt + 1}/5: {str(e)}. Retrying in 3 seconds...")
                    await asyncio.sleep(3)
                    continue
            
            raise e
    
    raise last_exception

def build_openrouter_content_with_image(text: str, image_bytes: bytes, mime_type: str = "image/jpeg") -> list:
    """Создаёт мультимодальный контент для OpenRouter с изображением"""
    image_base64 = base64.b64encode(image_bytes).decode('utf-8')
    data_url = f"data:{mime_type};base64,{image_base64}"
    
    return [
        {"type": "text", "text": text},
        {"type": "image_url", "image_url": {"url": data_url}}
    ]

def build_openrouter_content_with_audio(text: str, audio_bytes: bytes, audio_format: str = "wav") -> list:
    """Создаёт мультимодальный контент для OpenRouter с аудио"""
    audio_base64 = base64.b64encode(audio_bytes).decode('utf-8')
    
    return [
        {"type": "text", "text": text},
        {"type": "input_audio", "input_audio": {"data": audio_base64, "format": audio_format}}
    ]

async def openrouter_vision_analysis(
    image_bytes: bytes,
    text_prompt: str,
    system_message: str,
    mime_type: str = "image/jpeg"
) -> str:
    """Анализ изображения через OpenRouter с поддержкой ретраев"""
    content = build_openrouter_content_with_image(text_prompt, image_bytes, mime_type)
    
    messages = [
        {"role": "system", "content": system_message},
        {"role": "user", "content": content}
    ]
    
    return await openrouter_chat_with_retry(messages, max_tokens=4000, temperature=0.7)

async def openrouter_audio_transcription(
    audio_bytes: bytes,
    audio_format: str = "wav"
) -> str:
    """Транскрипция аудио через OpenRouter с поддержкой ретраев"""
    prompt = "Сделай точную транскрипцию речи из этого аудио файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это."
    content = build_openrouter_content_with_audio(prompt, audio_bytes, audio_format)
    
    messages = [
        {"role": "user", "content": content}
    ]
    
    return await openrouter_chat_with_retry(messages, max_tokens=8000, temperature=0.4)

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
    photo = update.message.photo
    logger.info(f"Handling message from user {user_id} ({user_name}). Text: '{text[:100]}...'. Document attached: {bool(document)}. Photo attached: {bool(photo)}") # Обновите сообщение журнала
    
    # Дополнительное логирование для отладки
    if photo:
        logger.info(f"Photo details: {len(photo)} sizes available. Largest: {photo[-1].width}x{photo[-1].height}")
    if update.message.caption:
        logger.info(f"Photo caption: '{update.message.caption}'")

    if photo: # Сначала обработайте фотографии
        logger.info(f"User {user_id} sent a photo. Calling handle_photo function.")
        await handle_photo(update, context)
        return

    if document: # Далее обработайте документы
        logger.warning(f"User {user_id} sent an unsupported document: {document.file_name}")
        await update.message.reply_text("Данный файл не поддерживается.")
        return

    # Если это не фотография и не документ, обрабатывайте как текстовое сообщение или команду
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

    # Обработка других текстовых команд и обычных сообщений
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

            # Конвертируем историю для Gemini API
            contents = []
            for message in chat_history_db:
                 # Пропускаем системное сообщение, так как оно передается отдельно
                 if message["role"] != "system":
                     if message["role"] == "user":
                         contents.append(types.UserContent(parts=[types.Part.from_text(text=message["content"])]))
                     else:  # assistant/model
                         contents.append(types.ModelContent(parts=[types.Part.from_text(text=message["content"])]))
            # Добавляем текущее сообщение пользователя
            contents.append(types.UserContent(parts=[types.Part.from_text(text=full_message)]))

            logger.info(f"Sending {len(contents)} contents (plus system instruction) to Gemini for user {user_id}.")
            response = await asyncio.to_thread(
                lambda: gemini_client.models.generate_content(
                    model=model_id,
                    contents=contents,
                    config=types.GenerateContentConfig(
                        system_instruction=system_message,
                        max_output_tokens=max_tokens,
                        temperature=1.0,
                        safety_settings=[
                            types.SafetySetting(category='HARM_CATEGORY_HARASSMENT', threshold='BLOCK_NONE'),
                            types.SafetySetting(category='HARM_CATEGORY_HATE_SPEECH', threshold='BLOCK_NONE'),
                            types.SafetySetting(category='HARM_CATEGORY_SEXUALLY_EXPLICIT', threshold='BLOCK_NONE'),
                        ]
                    )
                )
            )
            # Проверка на блокировку ответа
            if not response.text and hasattr(response, 'prompt_feedback') and response.prompt_feedback.block_reason:
                block_reason = response.prompt_feedback.block_reason
                logger.warning(f"Ответ Gemini заблокирован по причине: {block_reason}")
                bot_response = f"_(Ответ заблокирован Gemini по причине: {block_reason})_"
            else:
                bot_response = response.text

            logger.info(f"Received response from Gemini for user {user_id}.")

        elif provider == "openrouter":
            if openrouter_client is None or not OPENROUTER_API_KEY:
                raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")

            # Формируем сообщения для OpenRouter (формат OpenAI-совместимый)
            openrouter_messages = [{"role": "system", "content": system_message}]
            for message in chat_history_db:
                if message["role"] != "system":
                    openrouter_messages.append({
                        "role": message["role"],
                        "content": message["content"]
                    })
            openrouter_messages.append({"role": "user", "content": full_message})

            logger.info(f"Sending {len(openrouter_messages)} messages to OpenRouter for user {user_id}.")
            bot_response = await openrouter_chat_with_retry(
                messages=openrouter_messages,
                max_tokens=max_tokens,
                temperature=0.7
            )
            logger.info(f"Received response from OpenRouter for user {user_id}.")

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

# ADMIN_ID is now imported from config

@check_auth
async def handle_photo(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Processing photo from user {user_id} ({user_name}).")

    # Определяем выбранную модель и провайдера
    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    provider = MODELS[selected_model]["provider"]
    logger.info(f"Using provider '{provider}' for photo analysis (model: {selected_model})")

    # Проверяем доступность нужного клиента
    if provider == "openrouter":
        if not openrouter_client or not OPENROUTER_API_KEY:
            logger.error("OpenRouter client is not initialized.")
            await update.message.reply_text("Ошибка: Клиент OpenRouter не инициализирован.")
            return
    else:
        # Для всех остальных провайдеров используем Gemini для анализа изображений
        if not gemini_client:
            logger.error("Gemini client is not initialized.")
            await update.message.reply_text("Ошибка: Клиент Google Gemini не инициализирован.")
            return

    # Get the largest photo size
    photo_sizes = update.message.photo
    if not photo_sizes:
        logger.warning(f"No photo sizes found in message from user {user_id}.")
        await update.message.reply_text("Не удалось получить изображение.")
        return

    # The last photo size is usually the largest
    largest_photo = photo_sizes[-1]

    try:
        await update.message.chat.send_action(action=ChatAction.UPLOAD_PHOTO) # Показать действие загрузки фотографии
        photo_file = await largest_photo.get_file()
        photo_bytes = await photo_file.download_as_bytearray()

        logger.info(f"Photo downloaded from user {user_id}. Size: {len(photo_bytes)} bytes.")

        # Determine the text prompt. Use caption if available, otherwise a default prompt.
        text_prompt = update.message.caption or "Опиши это изображение."
        logger.info(f"Using text prompt for image analysis: '{text_prompt[:100]}...'")

        # Получаем пользовательский системный промпт
        user_prompt = get_user_prompt(user_id)
        system_message = user_prompt if user_prompt else SYSTEM_MESSAGE
        logger.info(f"Using system message for user {user_id}: '{system_message[:100]}...'")

        await update.message.chat.send_action(action=ChatAction.TYPING) # Show typing action while processing

        if provider == "openrouter":
            # Используем OpenRouter для анализа изображения
            logger.info(f"Sending image and prompt to OpenRouter for user {user_id}.")
            bot_response = await openrouter_vision_analysis(
                image_bytes=bytes(photo_bytes),
                text_prompt=text_prompt,
                system_message=system_message,
                mime_type="image/jpeg"
            )
            logger.info(f"Received response from OpenRouter for image analysis for user {user_id}. Snippet: '{bot_response[:100]}...'")
        else:
            # Используем Gemini с поддержкой fallback
            logger.info(f"Sending image and prompt to Gemini with fallback support for user {user_id}.")
            bot_response = await vision_analysis_with_model(
                photo_bytes=bytes(photo_bytes),
                text_prompt=text_prompt,
                system_message=system_message
            )
            logger.info(f"Received response from Gemini for image analysis for user {user_id}. Snippet: '{bot_response[:100]}...'")

        # Сохраняем сообщения в истории чата
        save_message(user_id, "user", f"[Изображение] {text_prompt}")
        save_message(user_id, "assistant", bot_response)

        formatted_response = format_text(bot_response) # Предполагается, что format_text доступен из utils
        message_parts = split_long_message(formatted_response) # Предполагается, что split_long_message доступен из utils

        for i, part in enumerate(message_parts):
            try:
                logger.info(f"Sending response part {i+1}/{len(message_parts)} to user {user_id}.")
                await update.message.reply_text(part, parse_mode=ParseMode.HTML)
            except BadRequest as e:
                logger.error(f"BadRequest error sending response part {i+1} to user {user_id}: {str(e)}. Sending raw text.")
                raw_part = re.sub('<[^<]+?>', '', part)
                try:
                    await update.message.reply_text(raw_part, parse_mode=None)
                except Exception as inner_e:
                    logger.error(f"Failed to send raw text part {i+1} as well for user {user_id}: {inner_e}")
                    await update.message.reply_text(f"Ошибка при отправке части ответа (попытка 2): {str(inner_e)}")
            except Exception as e:
                 logger.error(f"Unexpected error sending response part {i+1} to user {user_id}: {str(e)}", exc_info=True)
                 await update.message.reply_text(f"Ошибка при отправке части ответа: {str(e)}")

    except Exception as e:
        logger.error(f"Error processing photo for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке изображения: <code>{html.escape(str(e))}</code>", parse_mode=ParseMode.HTML)

@check_auth
async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Received voice message from user {user_id} ({user_name}).")
    
    # Определяем выбранную модель и провайдера
    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    provider = MODELS[selected_model]["provider"]
    logger.info(f"Using provider '{provider}' for voice processing (model: {selected_model})")
    
    temp_filename = f"tempvoice_{user_id}.wav"
    logger.info(f"Preparing to download voice to {temp_filename}.")

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем индикатор работы
        voice = await update.message.voice.get_file()
        voice_file = await voice.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(voice_file)
        logger.info(f"Voice message downloaded to {temp_filename}.")

        if provider == "openrouter" and openrouter_client and OPENROUTER_API_KEY:
            # Используем OpenRouter для транскрипции аудио
            logger.info(f"Transcribing using OpenRouter for user {user_id}...")
            recognized_text = await openrouter_audio_transcription(
                audio_bytes=bytes(voice_file),
                audio_format="wav"
            )
            logger.info(f"Voice message from user {user_id} ({user_name}) transcribed by OpenRouter: '{recognized_text}'")
        else:
            # Используем Gemini для транскрипции
            logger.info(f"Transcribing using Gemini for user {user_id}...")
            recognized_text = await audio_to_text(temp_filename, 'audio/wav')
            logger.info(f"Voice message from user {user_id} ({user_name}) transcribed by Gemini: '{recognized_text}'")

        # Проверяем, не вернула ли модель сообщение об ошибке
        if recognized_text.startswith("(Gemini):") or recognized_text.startswith("(OpenRouter):"):
            logger.warning(f"Transcription service returned a notice for user {user_id}: {recognized_text}")
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

@check_auth
@admin_required
async def list_users(update: Update, context: ContextTypes.DEFAULT_TYPE):
    admin_user_id = update.effective_user.id
    admin_user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Admin user {admin_user_id} ({admin_user_name}) requested list_users.")

    users = list_allowed_users()
    if not users:
        logger.info("No allowed users found.")
        await update.message.reply_text("Список разрешенных пользователей пуст.")
        return

    lines = [f"{user['telegram_id']} - {user['role']}" for user in users]
    message_text = "<b>Разрешенные пользователи:</b>\n" + "\n".join(lines)

    logger.info(f"Sending {len(users)} users to admin {admin_user_id}.")
    await update.message.reply_text(message_text, parse_mode=ParseMode.HTML)

@check_auth
@admin_required
async def list_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    admin_user_id = update.effective_user.id
    admin_user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Admin user {admin_user_id} ({admin_user_name}) requested list_user with args: {context.args}")

    if not context.args:
        await update.message.reply_text("Укажите ID пользователя. Пример: /list_user 123456789")
        return

    try:
        target_user_id = int(context.args[0])
    except ValueError:
        logger.warning(f"Invalid telegram_id provided to list_user by admin {admin_user_id}: {context.args}")
        await update.message.reply_text("ID пользователя должен быть числом. Пример: /list_user 123456789")
        return

    user_info = get_allowed_user(target_user_id)
    if not user_info:
        logger.info(f"User {target_user_id} not found in allowed list.")
        await update.message.reply_text(f"Пользователь {target_user_id} не найден в списке доступа.")
        return

    message_text = f"<b>ID:</b> {user_info['telegram_id']}\n<b>Роль:</b> {user_info['role']}"
    logger.info(f"Sending user info for {target_user_id} to admin {admin_user_id}.")
    await update.message.reply_text(message_text, parse_mode=ParseMode.HTML)

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
    
    # Определяем выбранную модель и провайдера
    selected_model = context.user_data.get('model', DEFAULT_MODEL)
    provider = MODELS[selected_model]["provider"]
    logger.info(f"Using provider '{provider}' for video processing (model: {selected_model})")
    
    temp_filename = f"tempvideo_{user_id}.mp4"
    logger.info(f"Preparing to download video to {temp_filename}.")

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING) # Показываем индикатор работы
        video = await update.message.video.get_file()
        video_bytes = await video.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(video_bytes)
        logger.info(f"Video message downloaded to {temp_filename}.")

        if provider == "openrouter" and openrouter_client and OPENROUTER_API_KEY:
            # Используем OpenRouter для транскрипции видео (аудио-дорожка)
            # Примечание: OpenRouter через Gemini поддерживает mp4 напрямую
            logger.info(f"Transcribing video audio using OpenRouter for user {user_id}...")
            recognized_text = await openrouter_audio_transcription(
                audio_bytes=bytes(video_bytes),
                audio_format="mp4"
            )
            logger.info(f"Video message from user {user_id} ({user_name}) transcribed by OpenRouter: '{recognized_text}'")
        else:
            # Используем Gemini для транскрипции
            logger.info(f"Transcribing video audio using Gemini for user {user_id}...")
            recognized_text = await audio_to_text(temp_filename, 'video/mp4')
            logger.info(f"Video message from user {user_id} ({user_name}) transcribed by Gemini: '{recognized_text}'")

        # Проверяем, не вернула ли модель сообщение об ошибке
        if recognized_text.startswith("(Gemini):") or recognized_text.startswith("(OpenRouter):"):
            logger.warning(f"Transcription service returned a notice for user {user_id}: {recognized_text}")
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

            (r"R2_ACCESS_KEY_ID=[^\s&]+", "R2_ACCESS_KEY_ID=[MASKED]"),
            (r"R2_SECRET_ACCESS_KEY=[^\s&]+", "R2_SECRET_ACCESS_KEY=[MASKED]"),
            (r"'aws_access_key_id': '[^']*'", "'aws_access_key_id': '[MASKED]'"),
            (r"'aws_secret_access_key': '[^']*'", "'aws_secret_access_key': '[MASKED]'")

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