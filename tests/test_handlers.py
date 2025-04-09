import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch, mock_open

# Добавляем импорты Voice и Video
from telegram import Update, User, Message, Chat, Voice, Video
from telegram.constants import ParseMode, ChatAction # Добавляем ChatAction
from telegram.ext import ContextTypes, Application, CommandHandler, MessageHandler, filters

import handlers
# Импортируем все необходимые функции и классы
from handlers import (
    start, clear, handle_message, handle_voice, handle_video, change_model,
    add_user, remove_user, process_message, get_main_keyboard,
    get_model_keyboard, get_extra_functions_keyboard, audio_to_text # Добавляем audio_to_text
)
from database import UserRole, get_user_role, add_allowed_user
from config import MODELS, DEFAULT_MODEL

pytestmark = pytest.mark.asyncio

@pytest.fixture
def mock_update():
    update = MagicMock(spec=Update)
    update.effective_user = MagicMock(spec=User)
    update.effective_user.id = 12345
    update.effective_user.username = 'testuser'
    update.effective_user.first_name = 'Test'
    update.message = MagicMock(spec=Message)
    update.message.chat = MagicMock(spec=Chat)
    update.message.chat.id = 123456
    update.message.reply_text = AsyncMock()
    update.message.chat.send_action = AsyncMock() # Мок для send_action
    # Инициализируем voice и video как None, тесты будут их переопределять
    update.message.voice = None
    update.message.video = None
    update.message.document = None
    update.message.text = ""
    update.message.caption = None
    return update

@pytest.fixture
def mock_context(mocker):
    context = MagicMock(spec=ContextTypes.DEFAULT_TYPE)
    context.user_data = {}
    context.bot = MagicMock()
    context.bot.send_message = AsyncMock()
    context.args = [] # Добавляем args для обработчиков команд
    return context

@pytest.fixture(autouse=True)
def mock_db_functions(mocker):
    mocker.patch('handlers.is_user_allowed', return_value=True) 
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) 
    mocker.patch('handlers.add_allowed_user')
    mocker.patch('handlers.remove_allowed_user')
    mocker.patch('handlers.clear_chat_history')
    mocker.patch('handlers.get_chat_history', return_value=[]) 
    mocker.patch('handlers.save_message')
    mocker.patch('handlers.update_user_prompt')
    mocker.patch('handlers.get_user_prompt', return_value=None) 
    mocker.patch('handlers.get_user_model', return_value=DEFAULT_MODEL) 
    mocker.patch('handlers.update_user_model')
    mocker.patch('handlers.set_user_auth_state')

@pytest.fixture(autouse=True)
def mock_api_clients_and_io(mocker):
    # --- Моки для генерации текста ---
    mock_groq_chat_create_method = AsyncMock()
    mock_groq_chat_create_method.return_value = MagicMock()
    mock_groq_chat_create_method.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Groq Response"))]
    mock_groq_client_instance = mocker.patch('handlers.groq_client', new_callable=AsyncMock, create=True)
    mock_groq_client_instance.chat.completions.create = mock_groq_chat_create_method

    # Mistral
    mock_mistral_complete_method = MagicMock() # Mistral SDK не async
    mock_mistral_complete_method.return_value = MagicMock()
    mock_mistral_complete_method.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Mistral Response"))]
    mock_mistral_client_instance = mocker.patch('handlers.mistral_client', new_callable=MagicMock, create=True)
    mock_mistral_client_instance.chat.complete = mock_mistral_complete_method

    # Gemini (генерация текста)
    mock_gemini_generate_async_method = AsyncMock() # Используем async версию
    mock_gemini_generate_async_method.return_value = MagicMock(text="Mocked Gemini Response")
    mock_generative_model_instance = MagicMock()
    mock_generative_model_instance.generate_content_async = mock_gemini_generate_async_method # Мокаем async метод
    # Мокаем и сам клиент, и метод GenerativeModel для возврата нашего мока модели
    mocker.patch('handlers.gemini_client', new_callable=MagicMock, create=True)
    mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_generative_model_instance, create=True)

    # --- Моки для транскрипции (теперь через audio_to_text) ---
    mocker.patch('handlers.audio_to_text', new_callable=AsyncMock, return_value="Mocked transcription text")

    # --- Моки для работы с файлами ---
    mock_download = AsyncMock(return_value=b'fake_file_content')
    mock_file_instance = MagicMock()
    mock_file_instance.download_as_bytearray = mock_download

    # Мокаем get_file для Voice и Video
    mocker.patch('telegram.Voice.get_file', AsyncMock(return_value=mock_file_instance))
    mocker.patch('telegram.Video.get_file', AsyncMock(return_value=mock_file_instance))

    # Моки для os и open
    mocker.patch('os.path.exists', return_value=True) # Предполагаем, что файл существует для удаления
    mocker.patch('os.remove')
    mocker.patch('builtins.open', mock_open()) # Мок для открытия/записи временных файлов

    # --- Моки для утилит форматирования ---
    # Используем side_effect для возврата исходного текста, чтобы не усложнять тесты
    mocker.patch('handlers.format_text', side_effect=lambda x: x)
    mocker.patch('handlers.split_long_message', side_effect=lambda x: [x] if x else [])


# --- Тесты Start ---
async def test_start_unauthorized(mock_update, mock_context, mocker):
    mocker.patch('handlers.is_user_allowed', return_value=False)
    mock_set_auth_state = mocker.patch('handlers.set_user_auth_state')

    await start(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, введите код авторизации:")
    mock_set_auth_state.assert_not_called() # Не должен устанавливать True

async def test_start_authorized(mock_update, mock_context, mocker):
    mocker.patch('handlers.is_user_allowed', return_value=True)
    # Используем другую модель для теста
    mocker.patch('handlers.get_user_model', return_value="Mistral Large 128K")
    mock_set_auth_state = mocker.patch('handlers.set_user_auth_state')

    await start(mock_update, mock_context)

    expected_text = f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>Mistral Large 128K</b>'
    mock_update.message.reply_text.assert_called_once()
    call_args, call_kwargs = mock_update.message.reply_text.call_args
    assert call_args[0] == expected_text
    assert call_kwargs['parse_mode'] == ParseMode.HTML
    assert call_kwargs['reply_markup'] is not None # Проверяем наличие клавиатуры
    assert mock_context.user_data['model'] == "Mistral Large 128K"
    mock_set_auth_state.assert_called_once_with(12345, True)

# --- Тесты Обработки Сообщений (Текст) ---
async def test_handle_message_text_gemini(mock_update, mock_context, mocker):
    # Настраиваем моки для этого теста
    mocker.patch('handlers.get_user_model', return_value="Gemini 2.0 Flash")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "user", "content": "previous message"}])
    # Получаем мок generate_content_async из фикстуры
    mock_gemini_generate_async = handlers.gemini_client.GenerativeModel.return_value.generate_content_async

    mock_update.message.text = "Привет, бот!"
    mock_context.user_data['model'] = "Gemini 2.0 Flash" # Устанавливаем модель в контексте

    await handle_message(mock_update, mock_context)

    # Проверки
    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Привет, бот!")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Gemini Response")

    mock_gemini_generate_async.assert_called_once()
    call_args, call_kwargs = mock_gemini_generate_async.call_args
    sent_messages = call_args[0]
    assert len(sent_messages) == 2 # previous + current (system prompt передается отдельно)
    assert sent_messages[0]['role'] == 'user' # Gemini ожидает user/model
    assert sent_messages[0]['parts'] == ["previous message"]
    assert sent_messages[1]['role'] == 'user'
    assert sent_messages[1]['parts'] == ["Привет, бот!"]

    # Проверяем отправку ответа и индикатора
    mock_update.message.reply_text.assert_called_once_with("Mocked Gemini Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)

# Тест для Mistral (аналогично Gemini)
async def test_handle_message_text_mistral(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_model', return_value="Mistral Large 128K")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "assistant", "content": "previous bot message"}])
    mock_mistral_complete = handlers.mistral_client.chat.complete

    mock_update.message.text = "Расскажи анекдот"
    mock_context.user_data['model'] = "Mistral Large 128K"

    await handle_message(mock_update, mock_context)

    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Расскажи анекдот")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Mistral Response")

    mock_mistral_complete.assert_called_once()
    call_args, call_kwargs = mock_mistral_complete.call_args
    sent_messages = call_kwargs['messages']
    assert len(sent_messages) == 3 # system + previous + current
    assert sent_messages[0]['role'] == 'system'
    assert sent_messages[1]['role'] == 'assistant'
    assert sent_messages[1]['content'] == "previous bot message"
    assert sent_messages[2]['role'] == 'user'
    assert sent_messages[2]['content'] == "Расскажи анекдот"

    mock_update.message.reply_text.assert_called_once_with("Mocked Mistral Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)


# --- Тесты Обработки Голоса (теперь с Gemini) ---
async def test_handle_voice_message(mock_update, mock_context, mocker):
    # Настраиваем мок для voice
    mock_voice = MagicMock(spec=Voice)
    mock_update.message.voice = mock_voice
    mock_update.message.text = None # Убедимся, что текста нет

    # Мок для process_message, чтобы проверить, что он вызывается с результатом транскрипции
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)
    # Получаем мок audio_to_text из фикстуры
    mock_audio_to_text = handlers.audio_to_text # Этот мок остается из фикстуры

    # Мокируем download_as_bytearray прямо здесь
    mock_download_method = mocker.patch('telegram.File.download_as_bytearray', new_callable=AsyncMock, return_value=b'fake_file_content')

    await handle_voice(mock_update, mock_context)

    # Проверяем вызов get_file (мок из фикстуры)
    mock_voice.get_file.assert_called_once()
    # Проверяем вызов download_as_bytearray (мок, созданный в этом тесте)
    mock_download_method.assert_called_once()

    # Проверяем вызов audio_to_text (мок из фикстуры)
    expected_filename = f"tempvoice_{mock_update.effective_user.id}.ogg"
    mock_audio_to_text.assert_called_once_with(expected_filename, 'audio/ogg')

    # Проверяем отправку индикатора
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)

    # Проверяем отправку промежуточного сообщения с транскрипцией
    mock_update.message.reply_text.assert_called_once_with(
        f"Распознано: \"Mocked transcription text\"\n\nОбрабатываю запрос..."
    )

    # Проверяем вызов process_message (мок, созданный в этом тесте)
    mock_process_message.assert_called_once_with(mock_update, mock_context, "Mocked transcription text")

    # Проверяем удаление временного файла (мок из фикстуры)
    handlers.os.remove.assert_called_once_with(expected_filename)

# --- Тесты Обработки Видео (теперь с Gemini) ---
async def test_handle_video_message(mock_update, mock_context, mocker):
    # Настраиваем мок для video
    mock_video = MagicMock(spec=Video)
    mock_update.message.video = mock_video
    mock_update.message.text = None
    mock_update.message.caption = None # Убедимся, что и caption пуст

    # Мок для process_message
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)
    # Получаем мок audio_to_text из фикстуры
    mock_audio_to_text = handlers.audio_to_text # Этот мок остается из фикстуры

    # Мокируем download_as_bytearray прямо здесь
    mock_download_method = mocker.patch('telegram.File.download_as_bytearray', new_callable=AsyncMock, return_value=b'fake_file_content')

    await handle_video(mock_update, mock_context)

    # Проверяем вызов get_file (мок из фикстуры)
    mock_video.get_file.assert_called_once()
    # Проверяем вызов download_as_bytearray (мок, созданный в этом тесте)
    mock_download_method.assert_called_once()

    # Проверяем вызов audio_to_text (мок из фикстуры)
    expected_filename = f"tempvideo_{mock_update.effective_user.id}.mp4"
    mock_audio_to_text.assert_called_once_with(expected_filename, 'video/mp4')

    # Проверяем отправку индикатора
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)

    # Проверяем отправку промежуточного сообщения
    mock_update.message.reply_text.assert_called_once_with(
        f"Распознано из видео: \"Mocked transcription text\"\n\nОбрабатываю запрос..."
    )

    # Проверяем вызов process_message (мок, созданный в этом тесте)
    mock_process_message.assert_called_once_with(mock_update, mock_context, "Mocked transcription text")

    # Проверяем удаление временного файла (мок из фикстуры)
    handlers.os.remove.assert_called_once_with(expected_filename)

# --- Тесты Очистки Контекста ---
async def test_clear_context(mock_update, mock_context, mocker):
    mock_clear_history = mocker.patch('handlers.clear_chat_history')
    # Устанавливаем любую модель, не важно какую
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)")

    mock_update.message.text = "Очистить контекст"

    # handle_message перенаправит на clear
    await handle_message(mock_update, mock_context)

    mock_clear_history.assert_called_once_with(12345)
    mock_update.message.reply_text.assert_called_once_with(
        '<b>История чата очищена.</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard() # Проверяем возврат главной клавиатуры
    )

# --- Тесты Смены Модели ---
async def test_change_model_show_options(mock_update, mock_context, mocker):
    mock_update.message.text = "Сменить модель"
    # handle_message перенаправит на change_model
    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with(
        'Выберите модель:',
        reply_markup=get_model_keyboard() # Проверяем клавиатуру выбора модели
    )

async def test_change_model_select_valid(mock_update, mock_context, mocker):
    selected_model = "Mistral Large 128K" # Выбираем другую модель
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model

    # handle_message перенаправит на change_model через regex
    await handle_message(mock_update, mock_context)

    mock_update_model_db.assert_called_once_with(12345, selected_model)
    assert mock_context.user_data['model'] == selected_model
    mock_update.message.reply_text.assert_called_once_with(
        f'Модель изменена на <b>{selected_model}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard() # Проверяем возврат главной клавиатуры
    )

# Тест на случай, если change_model вызывается напрямую (например, из regex)
async def test_change_model_direct_call(mock_update, mock_context, mocker):
    selected_model = "Llama 3.3 70B 8K (groq)"
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model # Имитируем текст сообщения = имени модели

    # Вызываем change_model напрямую
    await change_model(mock_update, mock_context)

    mock_update_model_db.assert_called_once_with(12345, selected_model)
    assert mock_context.user_data['model'] == selected_model
    mock_update.message.reply_text.assert_called_once_with(
        f'Модель изменена на <b>{selected_model}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

# --- Тесты Админских Команд ---
async def test_admin_add_user_success(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN) # Пользователь - админ
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["54321", "USER"] # Корректные аргументы

    await add_user(mock_update, mock_context)

    mock_add_db.assert_called_once_with(54321, UserRole.USER)
    mock_update.message.reply_text.assert_called_once_with("Пользователь 54321 успешно добавлен с ролью USER.")

async def test_admin_add_user_invalid_args(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["invalid_id"] # Некорректные аргументы

    await add_user(mock_update, mock_context)

    mock_add_db.assert_not_called()
    # Проверяем текст ошибки с примером
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER). Пример: /add_user 123456789 USER")

async def test_admin_add_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) # Пользователь НЕ админ
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["54321", "USER"]

    await add_user(mock_update, mock_context)

    mock_add_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("У вас нет прав для выполнения этой команды.")

async def test_admin_remove_user_success(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = ["54321"] # Корректный ID

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_called_once_with(54321)
    mock_update.message.reply_text.assert_called_once_with("Пользователь 54321 успешно удален.")

async def test_admin_remove_user_invalid_args(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = [] # Нет ID

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_not_called()
    # Проверяем текст ошибки с примером
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя. Пример: /remove_user 123456789")

async def test_admin_remove_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) # НЕ админ
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = ["54321"]

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("У вас нет прав для выполнения этой команды.")

# --- Тесты Редактирования Промпта ---
async def test_handle_message_edit_prompt_start(mock_update, mock_context, mocker):
    mock_update.message.text = "Изменить промпт"

    await handle_message(mock_update, mock_context)

    assert mock_context.user_data.get('editing_prompt') is True
    mock_update.message.reply_text.assert_called_once_with(
        "Введите новый системный промпт. Для отмены введите 'Назад':",
        reply_markup=get_extra_functions_keyboard() # Проверяем клавиатуру доп. функций
    )

async def test_handle_message_edit_prompt_submit(mock_update, mock_context, mocker):
    mock_update_prompt_db = mocker.patch('handlers.update_user_prompt')
    mock_context.user_data['editing_prompt'] = True # Включаем режим редактирования
    mock_update.message.text = "Новый системный промпт"

    await handle_message(mock_update, mock_context)

    mock_update_prompt_db.assert_called_once_with(12345, "Новый системный промпт")
    assert mock_context.user_data.get('editing_prompt') is False # Режим должен выключиться
    mock_update.message.reply_text.assert_called_once_with(
        "Системный промпт обновлен.",
        reply_markup=get_main_keyboard() # Возврат на главную клавиатуру
    )

async def test_handle_message_edit_prompt_cancel(mock_update, mock_context, mocker):
    mock_update_prompt_db = mocker.patch('handlers.update_user_prompt')
    mock_context.user_data['editing_prompt'] = True
    mock_update.message.text = "Назад"

    await handle_message(mock_update, mock_context)

    mock_update_prompt_db.assert_not_called() # Не должно быть вызова обновления
    assert mock_context.user_data.get('editing_prompt') is False
    mock_update.message.reply_text.assert_called_once_with(
        "Отмена обновления системного промпта.",
        reply_markup=get_main_keyboard() # Возврат на главную клавиатуру
    )

# --- Тесты Дополнительных Функций ---
async def test_handle_message_extra_functions(mock_update, mock_context, mocker):
    mock_update.message.text = "Доп функции"

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with(
        "Выберите действие:",
        reply_markup=get_extra_functions_keyboard() # Проверяем клавиатуру доп. функций
    )

async def test_handle_message_back_from_extra(mock_update, mock_context, mocker):
    mock_update.message.text = "Назад"
    # Убедимся, что мы НЕ в режиме редактирования промпта
    mock_context.user_data['editing_prompt'] = False

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with(
        'Выберите действие: (Или начните диалог)',
        reply_markup=get_main_keyboard() # Возврат на главную клавиатуру
    )

# --- Тест Неподдерживаемого Документа ---
async def test_handle_unsupported_document(mock_update, mock_context, mocker):
    # Настраиваем сообщение с документом
    mock_update.message.document = MagicMock()
    mock_update.message.document.file_name = "test.zip"
    mock_update.message.text = None # Текста нет

    # Мокаем process_message, чтобы убедиться, что он НЕ вызывается
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with("Данный файл не поддерживается.")
    mock_process_message.assert_not_called() # Убедимся, что обработка не пошла дальше

# --- Тест Обработки Ошибки Транскрипции ---
async def test_handle_voice_transcription_error(mock_update, mock_context, mocker):
    mock_voice = MagicMock(spec=Voice)
    mock_update.message.voice = mock_voice
    mock_update.message.text = None

    # Мокаем audio_to_text, чтобы он вызвал исключение
    error_message = "Ошибка Gemini API: Превышена квота"
    mocker.patch('handlers.audio_to_text', new_callable=AsyncMock, side_effect=Exception(error_message))
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)

    await handle_voice(mock_update, mock_context)

    # Проверяем, что process_message не был вызван
    mock_process_message.assert_not_called()

    # Проверяем сообщение об ошибке пользователю
    mock_update.message.reply_text.assert_called_once_with(
        f"Произошла ошибка при обработке голосового сообщения: {error_message}"
    )
    # Проверяем удаление файла даже при ошибке
    expected_filename = f"tempvoice_{mock_update.effective_user.id}.ogg"
    handlers.os.remove.assert_called_once_with(expected_filename)