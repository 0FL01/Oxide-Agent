import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch, mock_open

from telegram import Update, User, Message, Chat, Voice, Video
from telegram.constants import ParseMode
from telegram.ext import ContextTypes, Application, CommandHandler, MessageHandler, filters

from handlers import start, clear, handle_message, handle_voice, handle_video, change_model, add_user, remove_user, process_message, get_main_keyboard, get_model_keyboard, get_extra_functions_keyboard
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
    update.message.chat.send_action = AsyncMock()
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
    context.args = [] # Add args for command handlers
    return context

@pytest.fixture(autouse=True)
def mock_db_functions(mocker):
    mocker.patch('handlers.is_user_allowed', return_value=True) # Assume authorized by default
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) # Assume regular user by default
    mocker.patch('handlers.add_allowed_user')
    mocker.patch('handlers.remove_allowed_user')
    mocker.patch('handlers.clear_chat_history')
    mocker.patch('handlers.get_chat_history', return_value=[]) # Default empty history
    mocker.patch('handlers.save_message')
    mocker.patch('handlers.update_user_prompt')
    mocker.patch('handlers.get_user_prompt', return_value=None) # Default no custom prompt
    mocker.patch('handlers.get_user_model', return_value=DEFAULT_MODEL) # Default model
    mocker.patch('handlers.update_user_model')
    mocker.patch('handlers.set_user_auth_state') # Mock auth state setting

@pytest.fixture(autouse=True)
def mock_api_clients(mocker):
    mock_groq_chat_create_method = AsyncMock()
    mock_groq_chat_create_method.return_value = MagicMock()
    mock_groq_chat_create_method.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Groq Response"))]

    mock_groq_transcribe_method = AsyncMock()
    mock_groq_transcribe_method.return_value = MagicMock(text="Mocked transcription text")

    mock_groq_client_instance = mocker.patch('handlers.groq_client', new_callable=AsyncMock, create=True)
    mock_groq_client_instance.chat.completions.create = mock_groq_chat_create_method
    mock_groq_client_instance.audio.transcriptions.create = mock_groq_transcribe_method

    mock_mistral_complete_method = MagicMock()
    mock_mistral_complete_method.return_value = MagicMock()
    mock_mistral_complete_method.return_value.choices = [MagicMock(message=MagicMock(content="Mocked Mistral Response"))]

    mock_mistral_client_instance = mocker.patch('handlers.mistral_client', new_callable=MagicMock, create=True)
    mock_mistral_client_instance.chat.complete = mock_mistral_complete_method

    mock_gemini_generate_method = MagicMock()
    mock_gemini_generate_method.return_value = MagicMock(text="Mocked Gemini Response")

    mock_generative_model_instance = MagicMock()
    mock_generative_model_instance.generate_content = mock_gemini_generate_method

    mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_generative_model_instance, create=True)
    mocker.patch('handlers.gemini_client', new_callable=MagicMock, create=True)
    mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_generative_model_instance, create=True) # Re-patch on the top-level mock

    mock_download = AsyncMock(return_value=b'fake_file_content')
    mocker.patch('telegram.File.download_as_bytearray', mock_download)

    mock_file_instance = MagicMock()
    mock_file_instance.download_as_bytearray = mock_download

    mocker.patch('telegram.Voice.get_file', AsyncMock(return_value=mock_file_instance))
    mocker.patch('telegram.Video.get_file', AsyncMock(return_value=mock_file_instance))

    mocker.patch('os.path.exists', return_value=True)
    mocker.patch('os.remove')
    mocker.patch('builtins.open', mock_open())

    mocker.patch('handlers.format_text', side_effect=lambda x: x)
    mocker.patch('handlers.split_long_message', side_effect=lambda x: [x] if x else [])


async def test_start_unauthorized(mock_update, mock_context, mocker):
    mocker.patch('handlers.is_user_allowed', return_value=False)
    mock_set_auth_state = mocker.patch('handlers.set_user_auth_state')

    await start(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, введите код авторизации:")
    mock_set_auth_state.assert_not_called() # Should not set state to True

async def test_start_authorized(mock_update, mock_context, mocker):
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)")
    mock_set_auth_state = mocker.patch('handlers.set_user_auth_state')

    await start(mock_update, mock_context)

    expected_text = f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>Llama 3.3 70B 8K (groq)</b>'
    mock_update.message.reply_text.assert_called_once()
    call_args, call_kwargs = mock_update.message.reply_text.call_args
    assert call_args[0] == expected_text
    assert call_kwargs['parse_mode'] == ParseMode.HTML
    assert call_kwargs['reply_markup'] is not None
    assert mock_context.user_data['model'] == "Llama 3.3 70B 8K (groq)"
    mock_set_auth_state.assert_called_once_with(12345, True)

async def test_handle_message_text_gemini(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_model', return_value="Gemini 2.0 Flash")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "user", "content": "previous message"}])
    mock_gemini_generate = mocker.patch('handlers.gemini_client.GenerativeModel').return_value.generate_content

    mock_update.message.text = "Привет, бот!"
    mock_context.user_data['model'] = "Gemini 2.0 Flash"

    await handle_message(mock_update, mock_context)

    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Привет, бот!")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Gemini Response")

    mock_gemini_generate.assert_called_once()
    call_args, call_kwargs = mock_gemini_generate.call_args
    sent_messages = call_args[0]
    assert len(sent_messages) == 2 # previous + current
    assert sent_messages[0]['role'] == 'user'
    assert sent_messages[0]['parts'] == ["previous message"]
    assert sent_messages[1]['role'] == 'user'
    assert sent_messages[1]['parts'] == ["Привет, бот!"]

    mock_update.message.reply_text.assert_called_once_with("Mocked Gemini Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action='typing')

async def test_handle_message_text_groq(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_model', return_value="DeepSeek-R1-Distill-Llama-70B")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "user", "content": "previous"}])
    mock_groq_create = mocker.patch('handlers.groq_client.chat.completions.create')

    mock_update.message.text = "Hello Groq"
    mock_context.user_data['model'] = "DeepSeek-R1-Distill-Llama-70B"

    await handle_message(mock_update, mock_context)

    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Hello Groq")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Groq Response")

    mock_groq_create.assert_called_once()
    call_args, call_kwargs = mock_groq_create.call_args
    sent_messages = call_kwargs['messages']
    assert len(sent_messages) == 3 # system + history + current
    assert sent_messages[0]['role'] == 'system'
    assert sent_messages[1]['role'] == 'user'
    assert sent_messages[1]['content'] == 'previous'
    assert sent_messages[2]['role'] == 'user'
    assert sent_messages[2]['content'] == 'Hello Groq'

    mock_update.message.reply_text.assert_called_once_with("Mocked Groq Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action='typing')

async def test_handle_voice_message(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_model', return_value="DeepSeek-R1-Distill-Llama-70B")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_groq_transcribe = mocker.patch('handlers.groq_client.audio.transcriptions.create')
    mock_groq_chat_create = mocker.patch('handlers.groq_client.chat.completions.create')
    mock_os_remove = mocker.patch('os.remove')

    mock_update.message.voice = MagicMock(spec=Voice)

    await handle_voice(mock_update, mock_context)

    mock_groq_transcribe.assert_called_once()
    mock_groq_chat_create.assert_called_once()
    call_args, call_kwargs = mock_groq_chat_create.call_args
    messages = call_kwargs['messages']
    assert messages[-1]['role'] == 'user'
    assert messages[-1]['content'] == "Mocked transcription text"

    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Mocked transcription text")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Groq Response")

    mock_update.message.reply_text.assert_called_once_with("Mocked Groq Response", parse_mode=ParseMode.HTML)
    mock_os_remove.assert_called_once() # Check temp file removal

async def test_clear_context(mock_update, mock_context, mocker):
    mock_clear_history = mocker.patch('handlers.clear_chat_history')
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)")

    mock_update.message.text = "Очистить контекст"

    await handle_message(mock_update, mock_context) # handle_message routes to clear

    mock_clear_history.assert_called_once_with(12345)
    mock_update.message.reply_text.assert_called_once_with(
        '<b>История чата очищена.</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

async def test_handle_video_message(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_model', return_value="DeepSeek-R1-Distill-Llama-70B") # Use Groq for transcription
    mock_save_message = mocker.patch('handlers.save_message')
    mock_groq_transcribe = mocker.patch('handlers.groq_client.audio.transcriptions.create')
    mock_groq_chat_create = mocker.patch('handlers.groq_client.chat.completions.create')
    mock_os_remove = mocker.patch('os.remove')

    mock_update.message.video = MagicMock(spec=Video)
    mock_update.message.caption = None # Test without caption, should transcribe video

    await handle_video(mock_update, mock_context)

    mock_groq_transcribe.assert_called_once()
    mock_groq_chat_create.assert_called_once()
    call_args, call_kwargs = mock_groq_chat_create.call_args
    messages = call_kwargs['messages']
    assert messages[-1]['role'] == 'user'
    assert messages[-1]['content'] == "Mocked transcription text"

    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Mocked transcription text")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Groq Response")

    mock_update.message.reply_text.assert_called_once_with("Mocked Groq Response", parse_mode=ParseMode.HTML)
    mock_os_remove.assert_called_once()

async def test_change_model_show_options(mock_update, mock_context, mocker):
    mock_update.message.text = "Сменить модель"
    await handle_message(mock_update, mock_context) # Routes to change_model

    mock_update.message.reply_text.assert_called_once_with(
        'Выберите модель:',
        reply_markup=get_model_keyboard()
    )

async def test_change_model_select_valid(mock_update, mock_context, mocker):
    selected_model = "Mistral Large 128K"
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model

    await handle_message(mock_update, mock_context) # Routes to change_model via model name regex

    mock_update_model_db.assert_called_once_with(12345, selected_model)
    assert mock_context.user_data['model'] == selected_model
    mock_update.message.reply_text.assert_called_once_with(
        f'Модель изменена на <b>{selected_model}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

async def test_change_model_via_button(mock_update, mock_context, mocker):
    selected_model = "Llama 3.3 70B 8K (groq)"
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model # Simulate clicking the button with model name

    await change_model(mock_update, mock_context) # Direct call for button press simulation

    mock_update_model_db.assert_called_once_with(12345, selected_model)
    assert mock_context.user_data['model'] == selected_model
    mock_update.message.reply_text.assert_called_once_with(
        f'Модель изменена на <b>{selected_model}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

async def test_admin_add_user_success(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["54321", "USER"]

    await add_user(mock_update, mock_context)

    mock_add_db.assert_called_once_with(54321, UserRole.USER)
    mock_update.message.reply_text.assert_called_once_with("Пользователь 54321 успешно добавлен с ролью USER.")

async def test_admin_add_user_invalid_args(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["invalid"] # Missing role or invalid ID

    await add_user(mock_update, mock_context)

    mock_add_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER).")

async def test_admin_add_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) # Not an admin
    mock_add_db = mocker.patch('handlers.add_allowed_user')
    mock_context.args = ["54321", "USER"]

    await add_user(mock_update, mock_context)

    mock_add_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("У вас нет прав для выполнения этой команды.")

async def test_admin_remove_user_success(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = ["54321"]

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_called_once_with(54321)
    mock_update.message.reply_text.assert_called_once_with("Пользователь 54321 успешно удален.")

async def test_admin_remove_user_invalid_args(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.ADMIN)
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = [] # Missing ID

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя.")

async def test_admin_remove_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER) # Not an admin
    mock_remove_db = mocker.patch('handlers.remove_allowed_user')
    mock_context.args = ["54321"]

    await remove_user(mock_update, mock_context)

    mock_remove_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("У вас нет прав для выполнения этой команды.")

async def test_handle_message_edit_prompt_start(mock_update, mock_context, mocker):
    mock_update.message.text = "Изменить промпт"

    await handle_message(mock_update, mock_context)

    assert mock_context.user_data.get('editing_prompt') is True
    mock_update.message.reply_text.assert_called_once_with(
        "Введите новый системный промпт. Для отмены введите 'Назад':",
        reply_markup=get_extra_functions_keyboard()
    )

async def test_handle_message_edit_prompt_submit(mock_update, mock_context, mocker):
    mock_update_prompt_db = mocker.patch('handlers.update_user_prompt')
    mock_context.user_data['editing_prompt'] = True
    mock_update.message.text = "Новый системный промпт"

    await handle_message(mock_update, mock_context)

    mock_update_prompt_db.assert_called_once_with(12345, "Новый системный промпт")
    assert mock_context.user_data.get('editing_prompt') is False
    mock_update.message.reply_text.assert_called_once_with(
        "Системный промпт обновлен.",
        reply_markup=get_main_keyboard()
    )

async def test_handle_message_edit_prompt_cancel(mock_update, mock_context, mocker):
    mock_update_prompt_db = mocker.patch('handlers.update_user_prompt')
    mock_context.user_data['editing_prompt'] = True
    mock_update.message.text = "Назад"

    await handle_message(mock_update, mock_context)

    mock_update_prompt_db.assert_not_called()
    assert mock_context.user_data.get('editing_prompt') is False
    mock_update.message.reply_text.assert_called_once_with(
        "Отмена обновления системного промпта.",
        reply_markup=get_main_keyboard()
    )

async def test_handle_message_extra_functions(mock_update, mock_context, mocker):
    mock_update.message.text = "Доп функции"

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with(
        "Выберите действие:",
        reply_markup=get_extra_functions_keyboard()
    )

async def test_handle_message_back_from_extra(mock_update, mock_context, mocker):
    mock_update.message.text = "Назад"
    # Simulate being in the extra functions menu context (not editing prompt)
    mock_context.user_data['editing_prompt'] = False

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with(
        'Выберите действие: (Или начните диалог)',
        reply_markup=get_main_keyboard()
    )

async def test_handle_unsupported_document(mock_update, mock_context, mocker):
    mock_update.message.document = MagicMock()
    mock_update.message.document.file_name = "test.zip"
    mock_update.message.text = None # Ensure text is None when document is present

    await handle_message(mock_update, mock_context)

    mock_update.message.reply_text.assert_called_once_with("Данный файл не поддерживается.")
    # Ensure no API call or message processing happened
    mocker.patch('handlers.process_message').assert_not_called()