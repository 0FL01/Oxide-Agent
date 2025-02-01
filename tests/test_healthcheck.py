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