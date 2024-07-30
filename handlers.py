from telegram import Update, KeyboardButton, ReplyKeyboardMarkup, constants
from telegram.ext import ContextTypes
from config import chat_history, groq_client, octoai_client, MODELS, ADMIN_ID, search_tool
from utils import format_html, split_long_message, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from octoai.text_gen import ChatMessage
import logging
import os

logger = logging.getLogger(__name__)

def get_main_keyboard():
    keyboard = [
        [KeyboardButton("Очистить контекст"), KeyboardButton("Сменить модель")]
    ]
    return ReplyKeyboardMarkup(keyboard, resize_keyboard=True)

def get_model_keyboard():
    keyboard = [[KeyboardButton(model_name)] for model_name in MODELS.keys()]
    keyboard.append([KeyboardButton("Назад")])
    return ReplyKeyboardMarkup(keyboard, resize_keyboard=True)

def check_auth(func):
    async def wrapper(update: Update, context: ContextTypes.DEFAULT_TYPE):
        user_id = update.effective_user.id
        if not get_user_auth_state(user_id):
            await update.message.reply_text("Вы не авторизованы. Пожалуйста, введите /start для авторизации.")
            return
        return await func(update, context)
    return wrapper

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    logger.info(f"User {user_id} started the bot")

    if not is_user_allowed(user_id):
        await update.message.reply_text("Пожалуйста, введите код авторизации:")
        return

    if 'model' not in context.user_data:
        context.user_data['model'] = list(MODELS.keys())[0]

    set_user_auth_state(user_id, True)
    await update.message.reply_text(
        '<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь. Выберите действие:',
        parse_mode=constants.ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

@check_auth
async def clear(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    if user_id in chat_history:
        del chat_history[user_id]
    logger.info(f"Chat history cleared for user {user_id}")
    await update.message.reply_text('<b>История чата очищена.</b>', parse_mode=constants.ParseMode.HTML)

@check_auth
async def change_model(update: Update, context: ContextTypes.DEFAULT_TYPE):
    await update.message.reply_text(
        'Выберите модель:',
        reply_markup=get_model_keyboard()
    )

@check_auth
async def handle_message(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    text = update.message.text

    if text == "Очистить контекст":
        await clear(update, context)
        return

    if text == "Сменить модель":
        await change_model(update, context)
        return

    if text == "Назад":
        await update.message.reply_text(
            'Выберите действие:',
            reply_markup=get_main_keyboard()
        )
        return

    if text in MODELS:
        context.user_data['model'] = text
        await update.message.reply_text(
            f'Модель изменена на {text}. Выберите действие:',
            reply_markup=get_main_keyboard()
        )
        return

    await process_message(update, context, text)



async def process_message(update: Update, context: ContextTypes.DEFAULT_TYPE, text: str):
    user_id = update.effective_user.id
    
    if user_id not in chat_history:
        chat_history[user_id] = []

    chat_history[user_id].append({"role": "user", "content": text})
    chat_history[user_id] = chat_history[user_id][-10:]

    selected_model = context.user_data.get('model', list(MODELS.keys())[0])
    logger.info(f"Selected model for user {user_id}: {selected_model}")

    # Проверяем, нужен ли поиск
    need_search = len(text.split()) > 5 and not text.lower().startswith("перевод:")

    if need_search:
        try:
            search_results = search_tool.run(text[:100])  # Ограничиваем длину запроса
            search_response = f"Результаты поиска:\n\n{search_results}\n\n"
            chat_history[user_id].append({"role": "system", "content": search_response})
        except Exception as e:
            logger.error(f"Search error for user {user_id}: {str(e)}")
            search_response = "Не удалось выполнить поиск.\n\n"
            chat_history[user_id].append({"role": "system", "content": search_response})

    messages = [{"role": "system", "content": "Ты полезный ассистент, у тебя есть возможность искать информацию в интернете и на основе этих данных ты даёшь релевантный ответ. Используй следующие обозначения для форматирования: ** для жирного текста, * для курсива, также при работе с кодом, следуй стандартам отправки сообщений Telegram, * в начале строки для элементов списка."}] + chat_history[user_id]

    try:
        if MODELS[selected_model]["provider"] == "groq":
            response = await groq_client.chat.completions.create(
                messages=messages,
                model=MODELS[selected_model]["id"],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content
        elif MODELS[selected_model]["provider"] == "octoai":
            octoai_messages = [ChatMessage(content=msg["content"], role=msg["role"]) for msg in messages]
            response = octoai_client.text_gen.create_chat_completion(
                messages=octoai_messages,
                model=MODELS[selected_model]["id"],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content
        else:
            raise ValueError(f"Unknown provider for model {selected_model}")

        chat_history[user_id].append({"role": "assistant", "content": bot_response})
        logger.info(f"Sent response to user {user_id}")

        formatted_response = f"\n\n{format_html(bot_response)}"
        message_parts = split_long_message(formatted_response)
        
        for part in message_parts:
            await update.message.reply_text(part, parse_mode=constants.ParseMode.HTML)
    except Exception as e:
        logger.error(f"Error processing request for user {user_id}: {str(e)}")
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса: <code>{str(e)}</code>", parse_mode=constants.ParseMode.HTML)



async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
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
        logger.info(f"Voice message from user {user_id} recognized: {recognized_text}")

        await process_message(update, context, recognized_text)

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}")
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Temporary file {temp_filename} removed")



@check_auth
async def handle_voice(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
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
        logger.info(f"Voice message from user {user_id} recognized: {recognized_text}")

        if user_id not in chat_history:
            chat_history[user_id] = []

        chat_history[user_id].append({"role": "user", "content": recognized_text})
        chat_history[user_id] = chat_history[user_id][-10:]

        search_mode = context.user_data.get('search_mode', False)
        selected_model = context.user_data.get('model', list(MODELS.keys())[0])

        if search_mode:
            search_results = search_duckduckgo(recognized_text, max_results=3)
            search_response = "Вот что я нашёл в интернете:\n\n"
            for result in search_results:
                search_response += f"<b>{result['title']}</b>\n{result['href']}\n{result['body']}\n\n"
            chat_history[user_id].append({"role": "system", "content": search_response})

        messages = [{"role": "system", "content": "Ты полезный ассистент, у тебя есть возможность искать информацию в интернете и на основе этих данных ты даёшь релевантный ответ. Используй следующие обозначения для форматирования: ** для жирного текста, * для курсива, также при работе с кодом, следуй стандартам отправки сообщений Telegram, * в начале строки для элементов списка."}] + chat_history[user_id]

        try:
            if MODELS[selected_model]["provider"] == "groq":
                response = await groq_client.chat.completions.create(
                    messages=messages,
                    model=MODELS[selected_model]["id"],
                    temperature=0.7,
                    max_tokens=MODELS[selected_model]["max_tokens"],
                )
                bot_response = response.choices[0].message.content
            elif MODELS[selected_model]["provider"] == "octoai":
                octoai_messages = [ChatMessage(content=msg["content"], role=msg["role"]) for msg in messages]
                response = octoai_client.text_gen.create_chat_completion(
                    messages=octoai_messages,
                    model=MODELS[selected_model]["id"],
                    temperature=0.7,
                    max_tokens=MODELS[selected_model]["max_tokens"],
                )
                bot_response = response.choices[0].message.content
            else:
                raise ValueError(f"Unknown provider for model {selected_model}")

            chat_history[user_id].append({"role": "assistant", "content": bot_response})
            logger.info(f"Sent response to user {user_id}")

            formatted_response = f"\n\n{format_html(bot_response)}"
            message_parts = split_long_message(formatted_response)
            
            for part in message_parts:
                await update.message.reply_text(part, parse_mode=constants.ParseMode.HTML)
        except Exception as e:
            logger.error(f"Error processing request for user {user_id}: {str(e)}")
            await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса: <code>{str(e)}</code>", parse_mode=constants.ParseMode.HTML)

    except Exception as e:
        logger.error(f"Error processing voice message for user {user_id}: {str(e)}")
        await update.message.reply_text(f"Произошла ошибка при обработке голосового сообщения: {str(e)}")
    finally:
        if os.path.exists(temp_filename):
            os.remove(temp_filename)
            logger.info(f"Temporary file {temp_filename} removed")


@check_auth
async def add_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    if user_id != ADMIN_ID:
        await update.message.reply_text("У вас нет прав для выполнения этой команды.")
        return

    try:
        new_user_id = int(context.args[0])
        add_allowed_user(new_user_id)
        await update.message.reply_text(f"Пользователь {new_user_id} успешно добавлен.")
    except (ValueError, IndexError):
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя.")

@check_auth
async def remove_user(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    if user_id != ADMIN_ID:
        await update.message.reply_text("У вас нет прав для выполнения этой команды.")
        return

    try:
        remove_user_id = int(context.args[0])
        remove_allowed_user(remove_user_id)
        await update.message.reply_text(f"Пользователь {remove_user_id} успешно удален.")
    except (ValueError, IndexError):
        await update.message.reply_text("Пожалуйста, укажите корректный ID пользователя.")
