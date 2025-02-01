import os

# Устанавливаем фиктивные значения для всех ключей провайдеров
os.environ["GROQ_API_KEY"] = "dummy"
os.environ["HF_API_KEY"] = "dummy"
os.environ["OPENROUTER_API_KEY"] = "dummy"
os.environ["TOGETHER_API_KEY"] = "dummy"
os.environ["MISTRAL_API_KEY"] = "dummy"
os.environ["GITHUB_TOKEN"] = "dummy"
os.environ["GEMINI_API_KEY"] = "dummy"

import sys
# Добавляем корневую директорию в sys.path
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

import pytest
import asyncio
from handlers import healthcheck

# Создаем фиктивные объекты для имитации Telegram Update и Context

class DummyMessage:
    def __init__(self):
        self.replies = []

    async def reply_text(self, text, **kwargs):
        self.replies.append(text)

class DummyUser:
    def __init__(self, id=1):
        self.id = id

class DummyUpdate:
    def __init__(self):
        self.message = DummyMessage()
        self.effective_user = DummyUser()

class DummyContext:
    pass

@pytest.mark.asyncio
async def test_healthcheck():
    update = DummyUpdate()
    context = DummyContext()
    await healthcheck(update, context)
    assert update.message.replies == ["OK"] 