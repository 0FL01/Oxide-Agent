import os
import sys
import pytest
import asyncio
from unittest.mock import patch, Mock 

os.environ["ADMIN_ID"] = "1"
os.environ["TELEGRAM_TOKEN"] = "dummy"
os.environ["GROQ_API_KEY"] = "dummy"
os.environ["OPENROUTER_API_KEY"] = "dummy"
os.environ["MISTRAL_API_KEY"] = "dummy"
os.environ["GEMINI_API_KEY"] = "dummy"
os.environ["POSTGRES_DB"] = "test_db"
os.environ["POSTGRES_USER"] = "test_user"
os.environ["POSTGRES_PASSWORD"] = "test_password"
os.environ["POSTGRES_HOST"] = "localhost"
os.environ["POSTGRES_PORT"] = "5432"

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

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
    with patch('database.get_db_connection'):
        from handlers import healthcheck

        update = DummyUpdate()
        context = DummyContext()
        await healthcheck(update, context)
        assert update.message.replies == ["OK"]