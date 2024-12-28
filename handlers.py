from telegram import Update, KeyboardButton, ReplyKeyboardMarkup
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes
from config import chat_history, huggingface_client, azure_client, together_client, groq_client, openrouter_client, mistral_client, MODELS, encode_image, process_file, DEFAULT_MODEL, generate_image, gemini_client
from PIL import Image
from utils import split_long_message, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state, get_user_role, UserRole
from telegram.error import BadRequest
import html
from utils import format_html, split_long_message
import logging
import os
import re
import base64
import asyncio

PROMPT_IMPROVEMENT_SYSTEM_MESSAGE = """Ты — помощник, который улучшает пользовательские промпты для генерации изображений. Твоя цель — преобразовать запрос пользователя так, чтобы он был наиболее точным, детализированным и подходящим для генерации высококачественного изображения, сохраняя при этом изначальную суть запроса. Улучшенный промпт должен быть адаптирован на английский язык. Твоя задача — добавлять конкретные детали (например, описания объектов, цветов, поз, освещения, окружения, текстур и т.д.), которые помогут системе лучше понять и визуализировать запрос пользователя. Старайся сделать промпт максимально реалистичным, детализированным и ярким. Цензура не требуется, важно передать реалистичность и точность."""


logger = logging.getLogger(__name__)

SYSTEM_MESSAGE = """.  ###Answering Rules###

Follow in the strict order:

1. ИСПОЛЬЗУЙТЕ язык моего сообщения
2. В ПЕРВОМ сообщении перед ответом назначьте себе роль эксперта в реальном мире, например, «Я отвечу как всемирно известный эксперт по истории <подробная тема> с <самой престижной наградой за РЕАЛЬНОСТЬ МЕСТНОЙ темы>» или «Я отвечу как всемирно известный эксперт <конкретной науки> по <подробной теме> с <самой престижной наградой за МЕСТНУЮ тему>».
3. Вы ДОЛЖНЫ объединить свои глубокие знания темы и ясное мышление, чтобы быстро и точно расшифровать ответ шаг за шагом с КОНКРЕТНЫМИ деталями
4. За лучший ответ я дам чаевые в размере $1 000 000.
5. Ваш ответ имеет решающее значение для моей карьеры
6. Отвечайте на вопрос в естественной, человеческой манере
7. ВСЕГДА используйте ##пример ответа## для структуры первого сообщения.
8. Форматирование: - Оформите код в соответствии со стандартами Telegram: ``programming_language // ваш код здесь``.
И снова код должен быть экранирован тремя символами ```.

##Answering example##

// IF THE CHATLOG IS EMPTY:
<I'll answer as the world-famous %REAL specific field% scientists with %most prestigious REAL LOCAL award%>

**TL;DR**: <TL;DR, skip for rewriting>

<Step-by-step answer with CONCRETE details and key context>  ."""

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

    if 'model' not in context.user_data:
        context.user_data['model'] = list(MODELS.keys())[0]

    set_user_auth_state(user_id, True)
    await update.message.reply_text(
        '<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.',
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
async def handle_message(update: Update, context: ContextTypes.DEFAULT_TYPE):
    """
    Handle both text messages and documents, supporting multiple files in one message.
    """
    text = update.message.text or update.message.caption or ""
    image = update.message.photo[-1] if update.message.photo else None
    documents = update.message.document if isinstance(update.message.document, list) else [update.message.document] if update.message.document else []

    if text == "Очистить контекст":
        await clear(update, context)
    elif text == "Сменить модель":
        await change_model(update, context)
    elif text == "Назад":
        await update.message.reply_text(
            'Выберите действие: (Или начните диалог)',
            reply_markup=get_main_keyboard()
        )
    elif text in MODELS:
        context.user_data['model'] = text
        await update.message.reply_text(
            f'Модель изменена на <b>{text}</b>',
            parse_mode=ParseMode.HTML,
            reply_markup=get_main_keyboard()
        )
    elif documents:
        # Process all documents in the message
        combined_content = ""
        for doc in documents:
            file = await doc.get_file()
            file_extension = os.path.splitext(doc.file_name)[1].lower()
            file_path = f"temp_file_{update.effective_user.id}_{doc.file_name}"

            try:
                await file.download_to_drive(file_path)
                file_content = process_file(file_path)
                combined_content += f"\nСодержимое файла {doc.file_name}:\n{file_content}\n"
            finally:
                if os.path.exists(file_path):
                    os.remove(file_path)

        # Combine file content with user's text
        if combined_content:
            full_message = f"{combined_content}\nЗапрос пользователя: {text}"
            await process_message(update, context, full_message)
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

    if user_id not in chat_history:
        chat_history[user_id] = []

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
    
    chat_history[user_id].append({"role": "user", "content": full_message})
    chat_history[user_id] = chat_history[user_id][-10:]

    try:
        await update.message.chat.send_action(action=ChatAction.TYPING)


        if MODELS[selected_model]["provider"] == "groq":
            messages = [{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id]
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
                messages=[{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id],
                temperature=0.9,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "huggingface":
            if huggingface_client is None:
                raise ValueError("Huggingface client is not initialized. Please check your HF_API_KEY.")
            response = huggingface_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id],
                temperature=0.7,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "gemini":
            if gemini_client is None:
                raise ValueError("Gemini client is not initialized. Please check your GEMINI_API_KEY.")
            model = gemini_client.GenerativeModel(MODELS[selected_model]["id"])
            messages = [{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id]
            converted_messages = []
            for message in messages:
                converted_messages.append({
                    "role": "user" if message["role"] == "user" else "model",
                    "parts": [message["content"]]
                })

            # Добавляем изображение в запрос, если оно есть
            if image:
                image_data = Image.open(image_path)
                converted_messages.append({
                    "role": "user",
                    "parts": [image_data, text]
                })

            response = model.generate_content(
                converted_messages,
                generation_config=gemini_client.types.GenerationConfig(
                    max_output_tokens=MODELS[selected_model]["max_tokens"],
                    temperature=1,
                )
            )

            bot_response = response.text

        elif MODELS[selected_model]["provider"] == "together":
            if together_client is None:
                raise ValueError("Together AI client is not initialized. Please check your TOGETHER_API_KEY.")
            response = together_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id],
                temperature=0.8,
                max_tokens=MODELS[selected_model]["max_tokens"],
            )
            bot_response = response.choices[0].message.content

        elif MODELS[selected_model]["provider"] == "openrouter":
            if openrouter_client is None:
                raise ValueError("OpenRouter client is not initialized. Please check your OPENROUTER_API_KEY.")
            response = openrouter_client.chat.completions.create(
                model=MODELS[selected_model]["id"],
                messages=[{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id],
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

            messages = [{"role": "system", "content": SYSTEM_MESSAGE}] + chat_history[user_id]

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

        chat_history[user_id].append({"role": "assistant", "content": bot_response})
        logger.info(f"Sent response to user {user_id} ({user_name}): {bot_response}")

        formatted_response = format_html(bot_response)
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


async def improve_prompt(prompt: str, azure_client) -> str:
    messages = [
        {"role": "system", "content": PROMPT_IMPROVEMENT_SYSTEM_MESSAGE},
        {"role": "user", "content": f"{prompt}"}
    ]

    response = azure_client.chat.completions.create(
        model="gpt-4o-mini",
        messages=messages,
        temperature=1,
        max_tokens=500,
    )

    improved_prompt = response.choices[0].message.content
    return improved_prompt

async def generate_and_send_image(update: Update, context: ContextTypes.DEFAULT_TYPE, prompt: str):
    user_id = update.effective_user.id
    try:
        await update.message.chat.send_action(action=ChatAction.UPLOAD_PHOTO)

        # Улучшение промпта с помощью агента
        improved_prompt = await improve_prompt(prompt, azure_client)
        
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




