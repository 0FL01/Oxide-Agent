import nest_asyncio
nest_asyncio.apply()

import logging
import asyncio
from logging.handlers import TimedRotatingFileHandler
from telegram import Update
from telegram.ext import Application, CommandHandler, MessageHandler, filters
from handlers import start, clear, handle_message, handle_voice, change_model, add_user, remove_user, healthcheck, handle_video
from config import TELEGRAM_TOKEN, MODELS
import os
import re
from database import get_db_connection, check_postgres_connection, create_chat_history_table, create_user_models_table

class SensitiveDataFilter(logging.Filter):
    def __init__(self):
        super().__init__()
        self.patterns = [
            (r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)', r'\1[TELEGRAM_TOKEN]\3'),
            (r'([0-9]{8,10}:[A-Za-z0-9_-]{35})', '[TELEGRAM_TOKEN]'),
            (r'(bot[0-9]{8,10}:)[A-Za-z0-9_-]+', r'\1[TELEGRAM_TOKEN]')
        ]
        self.db_patterns = [
             (r"'user': '[^']*'", "'user': '[MASKED]'"),
             (r"'password': '[^']*'", "'password': '[MASKED]'"),
             (r"'dbname': '[^']*'", "'dbname': '[MASKED]'"),
             (r"'host': '[^']*'", "'host': '[MASKED]'"),
             (r"'port': '[^']*'", "'port': '[MASKED]'")
        ]


    def filter(self, record):
        if hasattr(record, 'msg'):
            if isinstance(record.msg, str):
                original_msg = record.msg
                for pattern, replacement in self.patterns:
                    record.msg = re.sub(pattern, replacement, record.msg)
                for pattern, replacement in self.db_patterns:
                     record.msg = re.sub(pattern, replacement, record.msg)

        if hasattr(record, 'args'):
            if record.args:
                args_list = list(record.args)
                for i, arg in enumerate(args_list):
                    if isinstance(arg, str):
                        original_arg = arg
                        for pattern, replacement in self.patterns:
                            args_list[i] = re.sub(pattern, replacement, args_list[i])
                        for pattern, replacement in self.db_patterns:
                            args_list[i] = re.sub(pattern, replacement, args_list[i])
                record.args = tuple(args_list)
        return True


class TokenMaskingFormatter(logging.Formatter):
    def __init__(self, fmt=None, datefmt=None):
        super().__init__(fmt, datefmt)
        self.sensitive_filter = SensitiveDataFilter()

    def format(self, record):
        self.sensitive_filter.filter(record)
        return super().format(record)

def setup_logging():
    if not os.path.exists('logs'):
        os.makedirs('logs')

    formatter = TokenMaskingFormatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(message)s',
        datefmt='%Y-%m-%d %H:%M:%S'
    )

    sensitive_filter = SensitiveDataFilter()

    file_handler = TimedRotatingFileHandler(
        'logs/acwl.log',
        when='h',
        interval=1,
        backupCount=72,
        encoding='utf-8'
    )
    file_handler.setFormatter(formatter)
    file_handler.addFilter(sensitive_filter)
    file_handler.setLevel(logging.INFO) 

    console_handler = logging.StreamHandler()
    console_handler.setFormatter(formatter)
    console_handler.addFilter(sensitive_filter)
    console_handler.setLevel(logging.INFO) 

    root_logger = logging.getLogger()
    root_logger.setLevel(logging.INFO) 
    for handler in root_logger.handlers[:]:
        root_logger.removeHandler(handler)
    root_logger.addHandler(file_handler)
    root_logger.addHandler(console_handler) 

    external_loggers = ['httpx', 'telegram', 'urllib3', 'psycopg2']
    for logger_name in external_loggers:
        ext_logger = logging.getLogger(logger_name)
        ext_logger.setLevel(logging.WARNING) 
        for handler in ext_logger.handlers[:]:
             ext_logger.removeHandler(handler)
        ext_logger.addHandler(file_handler)
        ext_logger.propagate = False

    return logging.getLogger(__name__)

logger = setup_logging()

async def main():
    try:
        logger.info("Starting the bot application")

        logger.info("Checking PostgreSQL network connectivity...")
        check_postgres_connection()

        logger.info("Initializing database tables...")
        create_chat_history_table()
        create_user_models_table()
        logger.info("Database tables initialized.")

        logger.info("Attempting test database connection...")
        try:
            with get_db_connection() as conn:
                with conn.cursor() as cur:
                    cur.execute("SELECT version();")
                    version = cur.fetchone()
                    logger.info(f"Successfully connected to PostgreSQL. Version: {version[0]}")
        except Exception as e:
            logger.error(f"Failed to establish test database connection during startup: {e}", exc_info=True)

        logger.info(f"Initializing Telegram Bot Application with token.") 
        application = Application.builder().token(TELEGRAM_TOKEN).build()
        logger.info("Telegram Bot Application initialized.")

        logger.info("Registering command handlers...")
        application.add_handler(CommandHandler("start", start))
        application.add_handler(CommandHandler("clear", clear))
        application.add_handler(CommandHandler("add_user", add_user))
        application.add_handler(CommandHandler("remove_user", remove_user))
        application.add_handler(CommandHandler("healthcheck", healthcheck))
        logger.info("Command handlers registered.")

        logger.info("Registering message handlers...")
        model_regex = f"^({'|'.join(re.escape(m) for m in MODELS)})$"
        application.add_handler(MessageHandler(
            filters.Regex("^Сменить модель$") | filters.Regex(model_regex),
            change_model
        ))
        logger.info("Model change handler registered.")

        application.add_handler(MessageHandler(
            filters.TEXT & ~filters.COMMAND & ~filters.Regex("^Сменить модель$") & ~filters.Regex(model_regex),
            handle_message
        ))
        logger.info("General text message handler registered.")

        application.add_handler(MessageHandler(filters.VOICE, handle_voice))
        logger.info("Voice message handler registered.")

        application.add_handler(MessageHandler(filters.VIDEO, handle_video))
        logger.info("Video message handler registered.")

        application.add_handler(MessageHandler(filters.Document.ALL, handle_message))
        logger.info("Document handler registered (handled within handle_message).")

        logger.info("All handlers registered.")

        logger.info("Starting bot polling...")
        await application.run_polling(allowed_updates=Update.ALL_TYPES)
        logger.info("Bot polling stopped.")

    except Exception as e:
        logger.critical(f"Critical error in main application loop: {e}", exc_info=True)
        raise 

if __name__ == '__main__':
    logger.info("Running main function...")
    asyncio.run(main())