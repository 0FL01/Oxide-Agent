# tests/test_handlers.py
import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch # Используем для мокинга

from telegram import Update, User, Message, Chat, Voice, Video
from telegram.ext import ContextTypes, Application, CommandHandler, MessageHandler, filters

from handlers import start, clear, handle_message, handle_voice, handle_video, change_model, add_user, remove_user, process_message
from database import UserRole, get_user_role, add_allowed_user
from config import MODELS, DEFAULT_MODEL

# Используем pytest-asyncio для асинхронных тестов
pytestmark = pytest.mark.asyncio

# --- Фикстуры для моков и настроек ---

@pytest.fixture
def mock_update(self):
    """Фикстура для создания мок-объекта Update."""
    update = MagicMock(spec=Update)
    update.effective_user = MagicMock(spec=User)
    update.effective_user.id = 12345 # Пример ID пользователя
    update.effective_user.username = 'testuser'
    update.effective_user.first_name = 'Test'
    update.message = MagicMock(spec=Message)
    update.message.chat = MagicMock(spec=Chat)
    update.message.chat.id = 123456 # Пример ID чата
    update.message.reply_text = AsyncMock() # Мокаем метод ответа
    update.message.chat.send_action = AsyncMock() # Мокаем отправку действия
    update.message.voice = None # По умолчанию нет голоса
    update.message.video = None # По умолчанию нет видео
    update.message.document = None # По умолчанию нет документа
    update.message.text = "" # По умолчанию текст пустой
    update.message.caption = None # По умолчанию подписи нет
    return update

@pytest.fixture
def mock_context(self, mocker):
    """Фикстура для создания мок-объекта Context."""
    context = MagicMock(spec=ContextTypes.DEFAULT_TYPE)
    context.user_data = {} # Имитируем user_data
    context.bot = MagicMock()
    context.bot.send_message = AsyncMock()
    # Мокаем API клиенты, если они используются через context (если нет - мокать при импорте)
    context.groq_client = AsyncMock() 
    context.mistral_client = MagicMock()
    context.gemini_client = MagicMock()
    return context

@pytest.fixture(autouse=True)
def mock_db_functions(self, mocker):
    """Фикстура для мокинга функций базы данных (альтернативный подход)."""
    pass

@pytest.fixture(autouse=True)
def mock_api_clients(mocker):
    """Фикстура для мокинга внешних API клиентов."""
    # --- Мокаем Groq ---
    mock_groq_chat_create = AsyncMock()
    mock_groq_chat_create.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Groq Response"))]
    mocker.patch('handlers.groq_client.chat.completions.create', mock_groq_chat_create, create=True)

    mock_groq_transcribe = AsyncMock()
    mock_groq_transcribe.return_value.text = "Mocked transcription text"
    mocker.patch('handlers.groq_client.audio.transcriptions.create', mock_groq_transcribe, create=True)   
    
    mock_groq_transcribe = AsyncMock()
    mock_groq_transcribe.return_value.text = "Mocked transcription text"
    mocker.patch('handlers.groq_client.audio.transcriptions.create', mock_groq_transcribe)
    mocker.patch('handlers.groq_client', new_callable=AsyncMock) # Мокаем сам клиент, если нужно

    # --- Мокаем Mistral ---
    mock_mistral_complete = MagicMock() 
    mock_mistral_complete.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Mistral Response"))]
    # Патчим в handlers
    mocker.patch('handlers.mistral_client.chat.complete', mock_mistral_complete, create=True)
    # Если нужно мокать сам клиент
    # mocker.patch('handlers.mistral_client', new_callable=MagicMock, create=True)

    # Мокаем Gemini
    mock_gemini_generate = MagicMock() # НЕ async, судя по коду handlers.py
    mock_gemini_generate.return_value.text = "Mocked Gemini Response"
    # Мокаем сложнее, т.к. там .GenerativeModel(...).generate_content(...)
    mock_generative_model = MagicMock()
    mock_generative_model.generate_content = mock_gemini_generate
    mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_generative_model)
    mocker.patch('handlers.gemini_client', MagicMock()) # Мокаем сам клиент

    # Мокаем скачивание файлов
    mock_download = AsyncMock(return_value=b'fake_file_content')
    mocker.patch('telegram.File.download_as_bytearray', mock_download)
    # Улучшенный мок для get_file, чтобы он возвращал мок файла с моком скачивания
    mock_file_instance = MagicMock()
    mock_file_instance.download_as_bytearray = mock_download
    mocker.patch('telegram.Voice.get_file', AsyncMock(return_value=mock_file_instance))
    mocker.patch('telegram.Video.get_file', AsyncMock(return_value=mock_file_instance))
    # Мокаем os функции для временных файлов
    mocker.patch('os.path.exists', return_value=True)
    mocker.patch('os.remove')
    mocker.patch('builtins.open', mocker.mock_open()) # Мокаем открытие/запись файлов

# --- Тесты для хендлеров ---

async def test_start_unauthorized(self, mock_update, mock_context, mocker):
    """Тест команды /start для неавторизованного пользователя."""
    # Настраиваем мок БД (если не используем тестовую БД)
    mocker.patch('handlers.is_user_allowed', return_value=False) 
    
    await start(mock_update, mock_context)
    
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, введите код авторизации:")
    # Проверяем, что состояние авторизации установлено в False
    assert mock_context.user_data.get(mock_update.effective_user.id) is False # Проверяем состояние в user_auth_states, если оно используется в тесте
    # Или через мок set_user_auth_state, если он доступен
    # mock_set_auth_state.assert_called_with(mock_update.effective_user.id, False)

async def test_start_authorized(self, mock_update, mock_context, mocker):
    """Тест команды /start для авторизованного пользователя."""
    # Настраиваем мок БД или тестовую БД
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)") # Пример сохраненной модели
    
    await start(mock_update, mock_context)
    
    expected_text = f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>Llama 3.3 70B 8K (groq)</b>'
    mock_update.message.reply_text.assert_called_once()
    call_args, call_kwargs = mock_update.message.reply_text.call_args
    assert call_args[0] == expected_text
    assert call_kwargs['parse_mode'] == 'HTML'
    # Проверяем, что клавиатура отправлена (нужно проверить структуру ReplyKeyboardMarkup)
    assert call_kwargs['reply_markup'] is not None 
    assert mock_context.user_data['model'] == "Llama 3.3 70B 8K (groq)"

async def test_handle_message_text(self, mock_update, mock_context, mocker):
    """Тест обработки обычного текстового сообщения."""
    mocker.patch('handlers.is_user_allowed', return_value=True) # Пользователь авторизован
    mocker.patch('handlers.get_user_model', return_value="Gemini 2.0 Flash") # Используем Gemini
    mocker.patch('handlers.get_chat_history', return_value=[{"role": "user", "content": "previous message"}]) # Пример истории
    mocker.patch('handlers.save_message') # Мокаем сохранение
    mocker.patch('handlers.format_text', side_effect=lambda x: x) # Простой мок форматирования
    mocker.patch('handlers.split_long_message', side_effect=lambda x: [x]) # Простой мок сплиттера
    
    mock_update.message.text = "Привет, бот!"
    
    await handle_message(mock_update, mock_context)
    
    # Проверяем, что было вызвано сохранение сообщения пользователя и ассистента
    assert mocker.patch('handlers.save_message').call_count == 2
    mocker.patch('handlers.save_message').assert_any_call(12345, "user", "Привет, бот!")
    mocker.patch('handlers.save_message').assert_any_call(12345, "assistant", "Mocked Gemini Response") # Ожидаем ответ от мока Gemini
    
    # Проверяем, что был вызван API Gemini (через мок)
    mock_generative_model = mocker.patch('handlers.gemini_client.GenerativeModel').return_value
    mock_generative_model.generate_content.assert_called_once()
    
    # Проверяем, что был отправлен ответ пользователю
    mock_update.message.reply_text.assert_called_once_with("Mocked Gemini Response", parse_mode='HTML')
    mock_update.message.chat.send_action.assert_called_once_with(action='typing')

async def test_handle_voice_message(self, mock_update, mock_context, mocker):
    """Тест обработки голосового сообщения."""
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.get_user_model', return_value="DeepSeek-R1-Distill-Llama-70B") # Используем Groq
    mocker.patch('handlers.save_message')
    mocker.patch('handlers.format_text', side_effect=lambda x: x)
    mocker.patch('handlers.split_long_message', side_effect=lambda x: [x])
    
    # Настраиваем мок для голосового сообщения
    mock_update.message.voice = MagicMock(spec=Voice)
    mock_update.message.voice.get_file = AsyncMock(return_value=MagicMock(download_as_bytearray=AsyncMock(return_value=b'fake_audio')))
    
    await handle_voice(mock_update, mock_context)
    
    # Проверяем вызов транскрипции
    mocker.patch('handlers.groq_client.audio.transcriptions.create').assert_called_once()
    
    # Проверяем вызов LLM (Groq) с распознанным текстом
    mocker.patch('handlers.groq_client.chat.completions.create').assert_called_once()
    call_args, call_kwargs = mocker.patch('handlers.groq_client.chat.completions.create').call_args
    messages = call_kwargs['messages']
    assert messages[-1]['role'] == 'user'
    assert messages[-1]['content'] == "Mocked transcription text" # Текст из мока транскрипции
    
    # Проверяем сохранение сообщения пользователя (распознанного) и ответа
    assert mocker.patch('handlers.save_message').call_count == 2
    mocker.patch('handlers.save_message').assert_any_call(12345, "user", "Mocked transcription text")
    mocker.patch('handlers.save_message').assert_any_call(12345, "assistant", "Mocked Groq Response")
    
    # Проверяем ответ пользователю
    mock_update.message.reply_text.assert_called_once_with("Mocked Groq Response", parse_mode='HTML')

# --- Тест команды /clear ---

async def test_clear_context(self, mock_update, mock_context, mocker):
    """Тест очистки контекста чата."""
    # Настраиваем моки
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.clear_chat_history', return_value=None)
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)")
    
    # Имитируем существующий контекст
    mock_context.user_data['model'] = "Llama 3.3 70B 8K (groq)"
    mock_context.user_data['history'] = [{"role": "user", "content": "test"}]
    
    await clear(mock_update, mock_context)
    
    # Проверяем вызов очистки истории
    mocker.patch('handlers.clear_chat_history').assert_called_once_with(12345)
    
    # Проверяем ответ
    mock_update.message.reply_text.assert_called_once_with(
        "История чата очищена. Текущая модель: <b>Llama 3.3 70B 8K (groq)</b>",
        parse_mode='HTML'
    )
    
    # Проверяем обновление user_data
    assert mock_context.user_data['history'] == []

# --- Тест обработки видео сообщения ---

async def test_handle_video_message(self, mock_update, mock_context, mocker):
    """Тест обработки видео сообщения."""
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.get_user_model', return_value="Gemini 2.0 Flash")
    mocker.patch('handlers.save_message')
    mocker.patch('handlers.format_text', side_effect=lambda x: x)
    mocker.patch('handlers.split_long_message', side_effect=lambda x: [x])
    
    # Настраиваем мок для видео
    mock_update.message.video = MagicMock(spec=Video)
    mock_update.message.video.get_file = AsyncMock(return_value=MagicMock(download_as_bytearray=AsyncMock(return_value=b'fake_video')))
    mock_update.message.caption = "Test caption"
    
    await handle_video(mock_update, mock_context)
    
    # Проверяем вызов Gemini с подписью
    mock_generative_model = mocker.patch('handlers.gemini_client.GenerativeModel').return_value
    mock_generative_model.generate_content.assert_called_once()
    call_args, call_kwargs = mock_generative_model.generate_content.call_args
    assert "Test caption" in call_args[0].parts[0].text
    
    # Проверяем сохранение сообщения
    assert mocker.patch('handlers.save_message').call_count == 2
    mocker.patch('handlers.save_message').assert_any_call(12345, "user", "Test caption")
    mocker.patch('handlers.save_message').assert_any_call(12345, "assistant", "Mocked Gemini Response")
    
    # Проверяем ответ
    mock_update.message.reply_text.assert_called_once_with("Mocked Gemini Response", parse_mode='HTML')

# --- Тесты смены модели ---

async def test_change_model_options(self, mock_update, mock_context, mocker):
    """Тест отображения вариантов моделей."""
    mocker.patch('handlers.is_user_allowed', return_value=True)
    
    await change_model(mock_update, mock_context)
    
    # Проверяем, что отправлены все доступные модели
    expected_text = "Выберите модель:\n" + "\n".join([f"/model_{i} - {model}" for i, model in enumerate(MODELS)])
    mock_update.message.reply_text.assert_called_once_with(expected_text)

async def test_change_model_select(self, mock_update, mock_context, mocker):
    """Тест выбора конкретной модели."""
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.update_user_model', return_value=None)
    
    # Имитируем выбор модели через аргументы команды
    mock_update.message.text = "/model_1"
    
    await change_model(mock_update, mock_context)
    
    # Проверяем обновление модели
    mocker.patch('handlers.update_user_model').assert_called_once_with(12345, MODELS[1])
    assert mock_context.user_data['model'] == MODELS[1]
    
    # Проверяем ответ
    mock_update.message.reply_text.assert_called_once_with(
        f"Модель изменена на: <b>{MODELS[1]}</b>",
        parse_mode='HTML'
    )

# --- Тесты админских команд ---

async def test_admin_add_user_success(self, mock_update, mock_context, mocker):
    """Тест успешного добавления пользователя админом."""
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mocker.patch('handlers.add_allowed_user', return_value=True)
    
    # Имитируем команду с username
    mock_update.message.text = "/add_user testuser2"
    
    await add_user(mock_update, mock_context)
    
    # Проверяем вызов добавления
    mocker.patch('handlers.add_allowed_user').assert_called_once_with("testuser2")
    
    # Проверяем ответ
    mock_update.message.reply_text.assert_called_once_with("Пользователь testuser2 добавлен")

async def test_admin_add_user_unauthorized(self, mock_update, mock_context, mocker):
    """Тест попытки добавления пользователя не-админом."""
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER)
    
    mock_update.message.text = "/add_user testuser2"
    
    await add_user(mock_update, mock_context)
    
    # Проверяем отказ
    mock_update.message.reply_text.assert_called_once_with("Эта команда только для администраторов")

async def test_admin_remove_user_success(self, mock_update, mock_context, mocker):
    """Тест успешного удаления пользователя админом."""
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mocker.patch('handlers.remove_allowed_user', return_value=True)
    
    # Имитируем команду с username
    mock_update.message.text = "/remove_user testuser2"
    
    await remove_user(mock_update, mock_context)
    
    # Проверяем вызов удаления
    mocker.patch('handlers.remove_allowed_user').assert_called_once_with("testuser2")
    
    # Проверяем ответ
    mock_update.message.reply_text.assert_called_once_with("Пользователь testuser2 удален")