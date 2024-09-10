from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
from config import chat_history, groq_client, octoai_client, openrouter_client, MODELS, ADMIN_ID, search_tool, user_settings, encode_image
from utils import split_long_message, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from octoai.text_gen import ChatMessage
import logging
import os
import re

logger = logging.getLogger(__name__)

SYSTEM_MESSAGE = """Ты высокоинтеллектуальный ИИ-ассистент с доступом к обширной базе знаний. Твоя задача - предоставлять точные, полезные и понятные ответы на вопросы пользователей, включая базовые и технические темы. Основные принципы: 1. Всегда стремись дать наиболее релевантный и точный ответ. 2. Если ты не уверен в ответе, честно сообщи об этом. 3. Используй простой язык, но не избегай технических терминов, когда они уместны. 4. При ответе на технические вопросы, старайся предоставить краткое объяснение и, если уместно, пример кода. Форматирование: - Используй **жирный текст** для выделения ключевых слов или фраз. - Используй *курсив* для определений или акцентирования. - Для списков используй * в начале строки. - Код оформляй в соответствии со стандартами Telegram: ```язык_программирования // твой код здесь ``` При ответе на вопросы: 1. Сначала дай краткий ответ. 2. Затем, если необходимо, предоставь более подробное объяснение. 3. Если уместно, приведи пример или предложи дополнительные ресурсы для изучения. Помни: твоя цель - помочь пользователю понять тему и решить его проблему."""

def get_main_keyboard():
    keyboard = [
        [KeyboardButton("Очистить контекст"), KeyboardButton("Сменить модель")],
        [KeyboardButton("Онлайн режим"), KeyboardButton("Оффлайн режим")]
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

def clean_html(text):
    """Remove any unclosed or improperly nested HTML tags, preserving code blocks."""
    # Temporarily replace code blocks
    code_blocks = []
    def replace_code_block(match):
        code_blocks.append(match.group(0))
        return f"__CODE_BLOCK_{len(code_blocks)-1}__"
    
    text = re.sub(r'```[\s\S]*?```', replace_code_block, text)
    
    # Remove any standalone < or > characters
    text = re.sub(r'(?<!<)>(?!>)', '&gt;', text)
    text = re.sub(r'(?<!<)<(?!<)', '&lt;', text)
    
    # Remove any unclosed tags
    open_tags = []
    clean_text = ""
    for char in text:
        if char == '<':
            open_tags.append(len(clean_text))
        elif char == '>':
            if open_tags:
                start = open_tags.pop()
                clean_text += text[start:len(clean_text)+1]
            else:
                clean_text += '&gt;'
        else:
            clean_text += char
    
    # Close any remaining open tags
    while open_tags:
        start = open_tags.pop()
        clean_text = clean_text[:start] + '&lt;' + clean_text[start+1:]
    
    # Restore code blocks
    for i, block in enumerate(code_blocks):
        clean_text = clean_text.replace(f"__CODE_BLOCK_{i}__", block)
    
    return clean_text

def format_html(text):
    text = clean_html(text)  # Clean the HTML first
    
    # Bold
    text = re.sub(r'\*\*(.*?)\*\*', r'<b>\1</b>', text)
    
    # Italic
    text = re.sub(r'\*(.*?)\*', r'<i>\1</i>', text)
    
    # Code blocks (three backticks)
    text = re.sub(r'```(\w+)?\n(.*?)\n```', r'<pre><code class="\1">\2</code></pre>', text, flags=re.DOTALL)
    
    # Inline code (single backticks)
    text = re.sub(r'`(.*?)`', r'<code>\1</code>', text)
    
    return text

async def start(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    logger.info(f"User {user_id} started the bot")

    if not is_user_allowed(user_id):
        await update.message.reply_text("Пожалуйста, введите код авторизации:")
        return

    if 'model' not in context.user_data:
        context.user_data['model'] = list(MODELS.keys())[0]

    if user_id not in user_settings:
        user_settings[user_id] = {'mode': 'offline'}

    set_user_auth_state(user_id, True)
    await update.message.reply_text(
        '<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

@check_auth
async def clear(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    if user_id in chat_history:
        del chat_history[user_id]
    logger.info(f"Chat history cleared for user {user_id}")
    await update.message.reply_text('<b>История чата очищена.</b>', parse_mode=ParseMode.HTML, reply_markup=get_main_keyboard())

@check_auth
async def change_model(update: Update, context: ContextTypes.DEFAULT_TYPE):
    await update.message.reply_text(
        'Выберите модель:',
        reply_markup=get_model_keyboard()
    )

@check_auth
async def set_online_mode(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_settings[user_id]['mode'] = 'online'
    context.user_data['model'] = "Gemma 2 9B-8192"
    await update.message.reply_text('Режим изменен на <b>онлайн</b>. Модель установлена на <b>Gemma 2 9B-8192</b>', parse_mode=ParseMode.HTML)

@check_auth
async def set_offline_mode(update: Update, context: ContextTypes.DEFAULT_TYPE):
    user_id = update.effective_user.id
    user_settings[user_id]['mode'] = 'offline'
    context.user_data['model'] = "Gemma 2 9B-8192"
    await update.message.reply_text('Режим изменен на <b>оффлайн</b>. Модель установлена на <b>Gemma 2 9B-8192</b>', parse_mode=ParseMode.HTML)


@check_auth
async def handle_message(update: Update, context: ContextTypes.DEFAULT_TYPE):
    text = update.message.text or ""
    image = update.message.photo[-1] if update.message.photo else None

    if text == "Очистить контекст":
        await clear(update, context)
    elif text == "Сменить модель":
        await change_model(update, context)
    elif text == "Онлайн режим":
        await set_online_mode(update, context)
    elif text == "Оффлайн режим":
        await set_offline_mode(update, context)
    elif text == "Назад":
        await update.message.reply_text(
            'Выберите действие:',
            reply_markup=get_main_keyboard()
        )
    elif text in MODELS:
        context.user_data['model'] = text
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    else:
        await process_message(update, context, text, image)


async def process_message(update: Update, context: ContextTypes.DEFAULT_TYPE, text: str, image=None):
    user_id = update.effective_user.id

    if user_id not in chat_history:
        chat_history[user_id] = []

    selected_model = context.user_data.get('model', list(MODELS.keys())[0])
    logger.info(f"Selected model for user {user_id}: {selected_model}")

    mode = user_settings[user_id]['mode']
    logger.info(f"Current mode for user {user_id}: {mode}")

    image_description = ""
    if image:
        file = await image.get_file()
        image_path = f"temp_image_{user_id}.jpg"
        await file.download_to_drive(image_path)
        image_base64 = encode_image(image_path)
        os.remove(image_path)

        llava_messages = [
            {"role": "user", "content": [
                {"type": "text", "text": "Describe this image in detail."},
                {"type": "image_url", "image_url": {"url": f"data:image/jpeg;base64,{image_base64}"}}
            ]}
        ]

        try:
            llava_response = await groq_client.chat.completions.create(
                messages=llava_messages,
                model="llava-v1.5-7b-4096-preview",
                temperature=0.7,
                max_tokens=1024,
            )
            image_description = llava_response.choices[0].message.content
            logger.info(f"Image description for user {user_id}: {image_description[:100]}...")
        except Exception as e:
            logger.error(f"Error processing image for user {user_id}: {str(e)}")
            image_description = "Не удалось обработать изображение."

    full_message = f"{text}\n\nОписание изображения: {image_description}" if image else text
    chat_history[user_id].append({"role": "user", "content": full_message})
    chat_history[user_id] = chat_history[user_id][-10:]

    search_response = ""
    if mode == 'online':
        need_search = len(text.split()) > 3 and not text.lower().startswith(("перевод:", "переведи:", "translate:"))
        if need_search:
            try:
                search_query = ' '.join(text.split()[:10])
                logger.info(f"Searching for: {search_query}")
                search_results = search_tool.run(search_query)
                search_response = f"Результаты поиска:\n\n{search_results}\n\n"
                chat_history[user_id].append({"role": "system", "content": search_response})
                logger.info(f"Search results for user {user_id}: {search_results[:100]}...")
            except Exception as e:
                logger.error(f"Search error for user {user_id}: {str(e)}")
                search_response = "Не удалось выполнить поиск.\n\n"
                chat_history[user_id].append({"role": "system", "content": search_response})

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING)

        if MODELS[selected_model]["provider"] == "groq":
            if selected_model == "Llava 1.5 7B-4096":
                messages = [{"role": "user", "content": full_message}]
            else:
                messages = [{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id]
            
            response = await groq_client.chat.completions.create(
                messages=messages,
                model=MODELS[selected_model]["id"],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content
        elif MODELS[selected_model]["provider"] == "octoai":
            octoai_messages = [ChatMessage(content=msg["content"], role=msg["role"]) for msg in [{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id]]
            response = octoai_client.text_gen.create_chat_completion(
                messages=octoai_messages,
                model=MODELS[selected_model]["id"],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content
        elif MODELS[selected_model]["provider"] == "openrouter":
            if openrouter_client is None:
                raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")
            response = openrouter_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id],
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
            await update.message.reply_text(part, parse_mode=ParseMode.HTML)
    except Exception as e:
        logger.error(f"Error processing request for user {user_id}: {str(e)}")
        await update.message.reply_text(f"<b>Ошибка:</b> Произошла ошибка при обработке вашего запроса: <code>{str(e)}</code>", parse_mode=ParseMode.HTML)


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

        await process_message(update, context, recognized_text)

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
