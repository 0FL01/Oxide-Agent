import logging
from logging.handlers import TimedRotatingFileHandler
from telegram.ext import Application, CommandHandler, MessageHandler, filters
from handlers import start, clear, handle_message, handle_voice, change_model, add_user, remove_user
from config import TELEGRAM_TOKEN
import os

# Создаем папку logs, если ее нет
if not os.path.exists('logs'):
    os.makedirs('logs')

# Настройка логирования в файл с ротацией по времени
logging.basicConfig(
    handlers=[TimedRotatingFileHandler('logs/acwl.log', when='h', interval=1, backupCount=72, encoding='utf-8')],
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    level=logging.INFO
)

logger = logging.getLogger(__name__)

def main():
    logger.info("Starting the bot")
    application = Application.builder().token(TELEGRAM_TOKEN).build()

    application.add_handler(CommandHandler("start", start))
    application.add_handler(CommandHandler("clear", clear))
    application.add_handler(CommandHandler("add_user", add_user))
    application.add_handler(CommandHandler("remove_user", remove_user))
    application.add_handler(MessageHandler(filters.TEXT | filters.PHOTO | filters.Document.ALL, handle_message))
    application.add_handler(MessageHandler(filters.VOICE, handle_voice))

    application.run_polling()

if __name__ == '__main__':
    main()
