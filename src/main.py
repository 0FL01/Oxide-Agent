import logging
import os
import re

from telegram import Update
from telegram.ext import Application, CommandHandler, MessageHandler, filters
from telegram.error import NetworkError, Conflict, TimedOut, RetryAfter, BadRequest

from config import TELEGRAM_TOKEN, MODELS
from database import check_r2_connection
from handlers import start, clear, handle_message, handle_voice, change_model, healthcheck, handle_video
from utils import TokenMaskingFormatter, SensitiveDataFilter


def setup_logging():
    formatter = TokenMaskingFormatter(
        '%(asctime)s - %(name)s - %(levelname)s - %(message)s',
        datefmt='%Y-%m-%d %H:%M:%S'
    )

    sensitive_filter = SensitiveDataFilter()

    console_handler = logging.StreamHandler()
    console_handler.setFormatter(formatter)
    console_handler.addFilter(sensitive_filter)
    console_handler.setLevel(logging.INFO)

    root_logger = logging.getLogger()
    root_logger.setLevel(logging.INFO)
    for handler in root_logger.handlers[:]:
        root_logger.removeHandler(handler)
    root_logger.addHandler(console_handler)

    external_loggers = ['httpx', 'telegram', 'urllib3']
    for logger_name in external_loggers:
        ext_logger = logging.getLogger(logger_name)
        ext_logger.setLevel(logging.WARNING)
        for handler in ext_logger.handlers[:]:
            ext_logger.removeHandler(handler)
        ext_logger.addHandler(console_handler)
        ext_logger.propagate = False

    return logging.getLogger(__name__)


logger = setup_logging()


async def error_handler(update: object, context) -> None:
    """Handle errors in the telegram bot.
    
    This handler catches and logs various types of errors:
    - NetworkError: Connection issues (retry automatically by polling)
    - TimedOut: Request timeout (retry automatically by polling)
    - Conflict: Multiple bot instances running (critical error)
    - RetryAfter: Rate limiting (wait and retry)
    - Other: Unexpected errors
    """
    error = context.error
    
    # Ignore NetworkError and TimedOut - they are handled by polling retry mechanism
    if isinstance(error, NetworkError):
        logger.warning(f"Network error occurred: {error}. Polling will retry automatically.")
        return
    
    if isinstance(error, TimedOut):
        logger.warning(f"Request timed out: {error}. Polling will retry automatically.")
        return
    
    # Conflict means another bot instance is running - this is a critical issue
    if isinstance(error, Conflict):
        logger.critical(
            f"Conflict error: {error}. "
            "Another bot instance is running with the same token. "
            "Please ensure only one bot instance is running."
        )
        # Don't raise - let the polling mechanism handle it
        return
    
    # RetryAfter - Telegram rate limiting
    if isinstance(error, RetryAfter):
        logger.warning(f"Rate limited. Retry after {error.retry_after} seconds.")
        return
    
    # BadRequest - usually a programming error or invalid data
    if isinstance(error, BadRequest):
        logger.error(f"Bad request error: {error}", exc_info=True)
        return
    
    # Log any other errors
    logger.error(f"Unhandled error occurred: {error}", exc_info=True)

def main():
    """Main function to run the Telegram bot."""
    try:
        logger.info("Starting the bot application")

        logger.info("Checking R2 storage connectivity...")
        if not check_r2_connection():
            logger.critical("Could not connect to R2 storage. Exiting.")
            return

        logger.info("R2 Storage connected.")

        logger.info("Initializing Telegram Bot Application with token.")
        application = Application.builder().token(TELEGRAM_TOKEN).build()
        logger.info("Telegram Bot Application initialized.")

        logger.info("Registering command handlers...")
        application.add_handler(CommandHandler("start", start))
        application.add_handler(CommandHandler("clear", clear))
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

        application.add_handler(MessageHandler(filters.PHOTO, handle_message))
        logger.info("Photo message handler registered.")

        logger.info("All handlers registered.")

        # Register error handler for graceful error handling
        application.add_error_handler(error_handler)
        logger.info("Error handler registered.")

        logger.info("Starting bot polling...")
        # Enhanced polling configuration for network resilience:
        # - drop_pending_updates: Clear old updates on start (helps with Conflict errors after restart)
        # - poll_interval: Time between polling requests (0 = immediate)
        # - timeout: Long polling timeout (default 10s is good for most cases)
        application.run_polling(
            allowed_updates=Update.ALL_TYPES,
            drop_pending_updates=True,
            poll_interval=0.0,
            timeout=30,
        )
        logger.info("Bot polling stopped.")

    except Exception as e:
        logger.critical(f"Critical error in main application loop: {e}", exc_info=True)
        raise


if __name__ == '__main__':
    logger.info("Running main function...")
    main()