import os
import sys
import pytest
import asyncio
from unittest.mock import patch, Mock 

os.environ["ADMIN_ID"] = "148349384"
os.environ["TELEGRAM_TOKEN"] = "dummy"
os.environ["GROQ_API_KEY"] = "dummy"
os.environ["OPENROUTER_API_KEY"] = "dummy"
os.environ["MISTRAL_API_KEY"] = "dummy"
os.environ["GEMINI_API_KEY"] = "dummy"
os.environ["R2_ENDPOINT_URL"] = "http://localhost"
os.environ["R2_ACCESS_KEY_ID"] = "test"
os.environ["R2_SECRET_ACCESS_KEY"] = "test"
os.environ["R2_BUCKET_NAME"] = "test"


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
    from handlers import healthcheck
    update = DummyUpdate()
    context = DummyContext()
    await healthcheck(update, context)
    assert update.message.replies == ["OK"]