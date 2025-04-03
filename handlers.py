from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
from config import chat_history, groq_client, openrouter_client, mistral_client, MODELS, process_file, DEFAULT_MODEL, gemini_client
from utils import split_long_message, clean_html, format_text
from database import UserRole, is_user_allowed, add_allowed_user, remove_allowed_user, get_user_role, clear_chat_history, get_chat_history, save_message, update_user_prompt, get_user_prompt, get_user_model, update_user_model
from telegram.error import BadRequest
import html
import logging
import os
import re
import asyncio
from dotenv import load_dotenv

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
    elif document:
        logger.info(f"Processing document '{document.file_name}' from user {user_id}.")
        file = await document.get_file()
        file_extension = os.path.splitext(document.file_name)[1].lower()
        file_path = f"temp_file_{user_id}_{document.file_name}"
        logger.info(f"Downloading document to {file_path}.")

        try:
            await file.download_to_drive(file_path)
            logger.info(f"Document downloaded. Processing file content.")
            file_content = process_file(file_path)
            logger.info(f"File content processed successfully.")
            full_message = f"\nСодержимое файла {document.file_name}:\n{file_content}\n"
            if text:
                full_message += f"\nЗапрос пользователя: {text}"
            else:
                 full_message += f"\nЗапрос пользователя: Опиши содержимое файла." # Добавляем запрос по умолчанию, если текст пуст
            logger.info(f"Sending document content and user query to process_message for user {user_id}.")
            await process_message(update, context, full_message) # Передаем собранное сообщение
        except ValueError as ve:
             logger.error(f"Value error processing document for user {user_id}: {ve}")
             await update.message.reply_text(f"Ошибка обработки файла: {str(ve)}")
        except Exception as e:
            logger.error(f"General error processing document for user {user_id}: {e}", exc_info=True)
            await update.message.reply_text(f"Произошла ошибка при обработке файла: {str(e)}")
        finally:
            if os.path.exists(file_path):
                os.remove(file_path)
                logger.info(f"Temporary file {file_path} removed for user {user_id}.")
    else:
        logger.info(f"Processing regular text message from user {user_id}.")
        await process_message(update, context, text)


async def process_document(update: Update, context: ContextTypes.DEFAULT_TYPE, document):
    user_id = update.effective_user.id
    user_name = update.effective_user.username or update.effective_user.first_name
    logger.info(f"Deprecated process_document called for user {user_id} ({user_name}) with file {document.file_name}. Should be handled by handle_message.")
    await handle_message(update, context)


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

        # --- ИСПРАВЛЕНИЕ НАЧАЛО ---
        # Добавляем текущее сообщение пользователя в список для отправки в API
        messages = [{"role": "system", "content": system_message}] + chat_history_db + [{"role": "user", "content": full_message}]
        # --- ИСПРАВЛЕНИЕ КОНЕЦ ---
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

            # Конвертируем сообщения в формат Gemini, включая системный промпт и текущее сообщение
            model = gemini_client.GenerativeModel(
                model_id,
                system_instruction=system_message # Явно передаем системный промпт
            )
            converted_messages = []
            for message in chat_history_db: # Только история
                 converted_messages.append({
                     "role": "user" if message["role"] == "user" else "model",
                     "parts": [message["content"]]
                 })
            # Добавляем текущее сообщение пользователя в конец
            converted_messages.append({"role": "user", "parts": [full_message]})

            logger.info(f"Sending {len(converted_messages)} converted messages (plus system prompt) to Gemini for user {user_id}.")
            response = model.generate_content(
                converted_messages, # Отправляем историю + текущее сообщение
                generation_config=gemini_client.types.GenerationConfig(
                    max_output_tokens=max_tokens,
                    temperature=1,
                )
            )
            bot_response = response.text
            logger.info(f"Received response from Gemini for user {user_id}.")


        elif provider == "openrouter":
            if openrouter_client is None:
                raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")
            logger.info(f"OpenRouter request payload for user {user_id}: model={model_id}, messages={messages}") # Добавлено логирование
            response = openrouter_client.chat.completions.create(
                model=model_id,
                messages=messages, # Теперь messages включает и текущий запрос
                temperature=0.8,
                max_tokens=max_tokens,
            )
            if response.choices and len(response.choices) > 0 and response.choices[0].message:
                bot_response = response.choices[0].message.content
                logger.info(f"Received response from OpenRouter for user {user_id}.")
            else:
                logger.error(f"Invalid response structure from OpenRouter for user {user_id}: {response}")
                raise ValueError("Опять API провайдер откис, воскреснет когда нибудь наверное")

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
                # Убираем экранирование html.unescape, т.к. format_text уже должен был подготовить текст
                await update.message.reply_text(part, parse_mode=None)
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
        voice = await update.message.voice.get_file()
        voice_file = await voice.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(voice_file)
        logger.info(f"Voice message downloaded to {temp_filename}. Transcribing...")

        with open(temp_filename, "rb") as audio_file:
            if groq_client is None:
                raise ValueError("Groq client is not initialized for transcription.")
            transcription = await groq_client.audio.transcriptions.create(
                file=(temp_filename, audio_file.read()),
                model="whisper-large-v3",
                language="ru"
            )

        recognized_text = transcription.text
        logger.info(f"Voice message from user {user_id} ({user_name}) transcribed: '{recognized_text}'")
        logger.info(f"Processing transcribed text for user {user_id}.")
        await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Temporary voice file {temp_filename} removed for user {user_id}.")


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
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER).")
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
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя.")
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
        video = await update.message.video.get_file()
        video_bytes = await video.download_as_bytearray()
        with open(temp_filename, "wb") as f:
            f.write(video_bytes)
        logger.info(f"Video message downloaded to {temp_filename}. Transcribing audio track...")

        with open(temp_filename, "rb") as video_file:
            if groq_client is None:
                 raise ValueError("Groq client is not initialized for transcription.")
            transcription = await groq_client.audio.transcriptions.create(
                file=(temp_filename, video_file.read()),
                model="whisper-large-v3",
                language="ru"
            )

        recognized_text = transcription.text
        logger.info(f"Video message from user {user_id} ({user_name}) transcribed: '{recognized_text}'")
        logger.info(f"Processing transcribed text from video for user {user_id}.")
        await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing video message for user {user_id}: {str(e)}", exc_info=True)
        await update.message.reply_text(f"Произошла ошибка при обработке видео сообщения: {str(e)}")

    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Temporary video file {temp_filename} removed for user {user_id}.")


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