# Another chat with LLM in telegram 

This Telegram bot leverages powerful language models to provide intelligent conversations, voice recognition, and internet search capabilities.

## Features

- Multiple language model support (Gemma 2 9B, Llama 3.1 70B, Llama 3.1 405B, and other model from OpenRouter.ai)
- Online and offline conversation modes
- Voice message transcription
- Internet search integration
- User authorization system
- Conversation history management
- Docker support for easy deployment

## Prerequisites

- Python 3.10+
- Telegram Bot Token
- Groq API Key
- OctoAI API Key

## Setup

1. Clone the repository:
   ```
   git clone https://github.com/0FL01/Another-Chat-with-LLM.git
   cd Another-Chat-with-LLM
   ```

2. Create a `.env` file in the project root and add your API keys:
   ```
   GROQ_API_KEY=your_groq_api_key
   TELEGRAM_TOKEN=your_telegram_bot_token
   ADMIN_ID=your_admin_telegram_id
   OCTOAI_API_KEY=your_octoai_api_key
   OPENROUTER_API_KEY=your_openrouter_api_key
   ```

3. Install dependencies:
   ```
   pip install -r requirements.txt
   ```

## Running the Bot

### Local Deployment

Run the bot using Python:

```
python watchdog_runner.py
```

### Docker Deployment

1. Build and run the Docker container:
   ```
   docker-compose up -d
   ```

2. To stop the bot:
   ```
   docker-compose down
   ```

## Usage

- Start the bot by sending `/start` in Telegram
- Choose between online and offline modes
- Send text messages or voice messages for responses
- Clear conversation context or change the AI model as needed

## Admin Commands

- Add a user: `/add_user <user_id>`
- Remove a user: `/remove_user <user_id>`

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the GNU GENERAL PUBLIC LICENSE Version 3 - see the LICENSE file for details.
