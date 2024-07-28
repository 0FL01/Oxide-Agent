import logging
from telegram.ext import Application, CommandHandler, MessageHandler, filters, ConversationHandler
from handlers import start, clear, handle_message_with_mode, handle_voice, select_model, handle_auth, add_user, remove_user, CHOOSING, SELECTING_MODEL, AWAITING_AUTH
from config import TELEGRAM_TOKEN

logging.basicConfig(
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    level=logging.INFO
)
logger = logging.getLogger(__name__)

def main():
    logger.info("Starting the bot")
    application = Application.builder().token(TELEGRAM_TOKEN).build()

    conv_handler = ConversationHandler(
        entry_points=[CommandHandler("start", start)],
        states={
            CHOOSING: [
                MessageHandler(filters.TEXT & ~filters.COMMAND, handle_message_with_mode),
                MessageHandler(filters.VOICE, handle_voice),
            ],
            SELECTING_MODEL: [
                MessageHandler(filters.TEXT & ~filters.COMMAND, select_model),
            ],
            AWAITING_AUTH: [
                MessageHandler(filters.TEXT & ~filters.COMMAND, handle_auth),
            ],
        },
        fallbacks=[CommandHandler("start", start)],
    )

    application.add_handler(conv_handler)
    application.add_handler(CommandHandler("clear", clear))
    application.add_handler(CommandHandler("add_user", add_user))
    application.add_handler(CommandHandler("remove_user", remove_user))

    application.run_polling()

if __name__ == '__main__':
    main()
