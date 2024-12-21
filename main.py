import logging
from logging.handlers import TimedRotatingFileHandler
from telegram.ext import Application, CommandHandler, MessageHandler, filters
from handlers import start, clear, handle_message, handle_voice, change_model, add_user, remove_user
from config import TELEGRAM_TOKEN
import os
import re

# Создаем папку logs, если ее нет
if not os.path.exists('logs'):
    os.makedirs('logs')

class TokenMaskingFormatter(logging.Formatter):
    def __init__(self, fmt=None, datefmt=None):
        super().__init__(fmt, datefmt)
        
    def format(self, record):
        if isinstance(record.msg, str):
            # Более точный паттерн для поиска URL с токеном
            record.msg = re.sub(
                r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)',
                r'\1HIDDEN_TOKEN\3',
                record.msg
            )
        if record.args:
            record.args = tuple(
                re.sub(
                    r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)',
                    r'\1HIDDEN_TOKEN\3',
                    arg
                )
                if isinstance(arg, str) else arg
                for arg in record.args
            )
        return super().format(record)

# Настройка форматтера и хендлера
formatter = TokenMaskingFormatter('%(asctime)s - %(name)s - %(levelname)s - %(message)s')
file_handler = TimedRotatingFileHandler(
    'logs/acwl.log',
    when='h',
    interval=1,
    backupCount=72,
    encoding='utf-8'
)
file_handler.setFormatter(formatter)

# Настройка корневого логгера
root_logger = logging.getLogger()
root_logger.setLevel(logging.INFO)
root_logger.addHandler(file_handler)

# Настройка логгера для httpx
httpx_logger = logging.getLogger('httpx')
httpx_logger.setLevel(logging.INFO)
# Удаляем существующие хендлеры
for handler in httpx_logger.handlers:
    httpx_logger.removeHandler(handler)
httpx_logger.addHandler(file_handler)
httpx_logger.propagate = False

# Настройка логгера для приложения
logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)

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
