import pytest
import asyncio
from unittest.mock import AsyncMock, MagicMock, patch, mock_open, call, ANY
import os
import sys
import google.api_core.exceptions

import telegram
from telegram import Update, User, Message, Chat, Voice, Video, File
from telegram.constants import ParseMode, ChatAction
from telegram.ext import ContextTypes

# Добавляем импорт mock (или используем ANY напрямую из unittest.mock)
from unittest import mock

import handlers
from handlers import (
    start, clear, handle_message, handle_voice, handle_video, change_model,
    add_user, remove_user, process_message, get_main_keyboard,
    get_model_keyboard, get_extra_functions_keyboard, audio_to_text,
    MAX_RETRIES, RETRY_DELAY_SECONDS
)
from database import UserRole
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
    context.args = []
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
    mock_groq_chat_create_method = AsyncMock(return_value=MagicMock(choices=[MagicMock(message=MagicMock(content="Mocked Groq Response"))]))
    mock_groq_client_instance = mocker.patch('handlers.groq_client', new_callable=AsyncMock, create=True)
    if mock_groq_client_instance:
        mock_groq_client_instance.chat.completions.create = mock_groq_chat_create_method

    mock_mistral_complete_method = MagicMock(return_value=MagicMock(choices=[MagicMock(message=MagicMock(content="Mocked Mistral Response"))]))
    mock_mistral_client_instance = mocker.patch('handlers.mistral_client', new_callable=MagicMock, create=True)
    if mock_mistral_client_instance:
        mock_mistral_client_instance.chat.complete = mock_mistral_complete_method

    mock_gemini_generate_async_method = AsyncMock(return_value=MagicMock(text="Mocked Gemini Response"))
    mock_generative_model_instance = MagicMock()
    mock_generative_model_instance.generate_content_async = mock_gemini_generate_async_method
    mock_gemini_client_instance = mocker.patch('handlers.gemini_client', new_callable=MagicMock, create=True)
    if mock_gemini_client_instance:
        mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_generative_model_instance, create=True)

    mocker.patch('handlers.audio_to_text', new_callable=AsyncMock, return_value="Mocked transcription text")

    mock_download_method = AsyncMock(return_value=b'fake_file_content')
    mock_file_instance = MagicMock(spec=File)
    mock_file_instance.download_as_bytearray = mock_download_method

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
    mock_set_auth_state.assert_not_called()

async def test_start_authorized(mock_update, mock_context, mocker):
    mocker.patch('handlers.is_user_allowed', return_value=True)
    mocker.patch('handlers.get_user_model', return_value="Mistral Large 128K")
    mock_set_auth_state = mocker.patch('handlers.set_user_auth_state')
    await start(mock_update, mock_context)
    expected_text = f'<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>Mistral Large 128K</b>'
    mock_update.message.reply_text.assert_called_once_with(
        expected_text,
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )
    assert mock_context.user_data['model'] == "Mistral Large 128K"
    mock_set_auth_state.assert_called_once_with(12345, True)

async def test_handle_message_text_gemini(mock_update, mock_context, mocker):
    if not handlers.gemini_client:
        pytest.skip("Gemini client is not mocked/available")
    mocker.patch('handlers.get_user_model', return_value="Gemini 2.0 Flash")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "user", "content": "previous"}])
    mock_gemini_generate_async = handlers.gemini_client.GenerativeModel.return_value.generate_content_async
    mock_update.message.text = "Привет!"
    mock_context.user_data['model'] = "Gemini 2.0 Flash"

    await handle_message(mock_update, mock_context)

    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Привет!")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Gemini Response")
    mock_gemini_generate_async.assert_called_once()
    call_args, call_kwargs = mock_gemini_generate_async.call_args # Исправлено: call_args это объект, а не кортеж
    assert call_args[0] == [{"role": "user", "parts": ["previous"]}, {"role": "user", "parts": ["Привет!"]}] # Исправлено: доступ к args через call_args[0] или call_args.args
    mock_update.message.reply_text.assert_called_once_with("Mocked Gemini Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)

async def test_handle_message_text_mistral(mock_update, mock_context, mocker):
    if not handlers.mistral_client:
         pytest.skip("Mistral client is not mocked/available")
    mocker.patch('handlers.get_user_model', return_value="Mistral Large 128K")
    mock_save_message = mocker.patch('handlers.save_message')
    mock_get_history = mocker.patch('handlers.get_chat_history', return_value=[{"role": "assistant", "content": "prev bot"}])
    mock_mistral_complete = handlers.mistral_client.chat.complete
    mock_update.message.text = "Анекдот?"
    mock_context.user_data['model'] = "Mistral Large 128K"

    await handle_message(mock_update, mock_context)

    mock_get_history.assert_called_once_with(12345)
    assert mock_save_message.call_count == 2
    mock_save_message.assert_any_call(12345, "user", "Анекдот?")
    mock_save_message.assert_any_call(12345, "assistant", "Mocked Mistral Response")
    mock_mistral_complete.assert_called_once()
    call_args, call_kwargs = mock_mistral_complete.call_args # Исправлено: call_args это объект
    assert call_kwargs['messages'][0]['role'] == 'system'
    assert call_kwargs['messages'][1] == {"role": "assistant", "content": "prev bot"}
    assert call_kwargs['messages'][2] == {"role": "user", "content": "Анекдот?"}
    mock_update.message.reply_text.assert_called_once_with("Mocked Mistral Response", parse_mode=ParseMode.HTML)
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)

async def test_handle_voice_message(mock_update, mock_context, mocker):
    mock_voice = MagicMock(spec=Voice)
    mock_file_instance = await telegram.Voice.get_file()
    mock_voice.get_file = AsyncMock(return_value=mock_file_instance)
    mock_update.message.voice = mock_voice
    mock_update.message.text = None
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)
    mock_audio_to_text = handlers.audio_to_text

    await handle_voice(mock_update, mock_context)

    mock_voice.get_file.assert_called_once()
    mock_file_instance.download_as_bytearray.assert_called_once()

    expected_filename = f"tempvoice_{mock_update.effective_user.id}.ogg"
    mock_audio_to_text.assert_called_once_with(expected_filename, 'audio/ogg')
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)
    mock_update.message.reply_text.assert_called_once_with(
        f"Распознано: \"Mocked transcription text\"\n\nОбрабатываю запрос..."
    )
    mock_process_message.assert_called_once_with(mock_update, mock_context, "Mocked transcription text")
    handlers.os.remove.assert_called_once_with(expected_filename)


async def test_handle_video_message(mock_update, mock_context, mocker):
    mock_video = MagicMock(spec=Video)
    mock_file_instance = await telegram.Video.get_file()
    mock_video.get_file = AsyncMock(return_value=mock_file_instance)
    mock_update.message.video = mock_video
    mock_update.message.text = None
    mock_update.message.caption = None
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)
    mock_audio_to_text = handlers.audio_to_text

    await handle_video(mock_update, mock_context)

    mock_video.get_file.assert_called_once()
    mock_file_instance.download_as_bytearray.assert_called_once()

    expected_filename = f"tempvideo_{mock_update.effective_user.id}.mp4"
    mock_audio_to_text.assert_called_once_with(expected_filename, 'video/mp4')
    mock_update.message.chat.send_action.assert_called_once_with(action=ChatAction.TYPING)
    mock_update.message.reply_text.assert_called_once_with(
        f"Распознано из видео: \"Mocked transcription text\"\n\nОбрабатываю запрос..."
    )
    mock_process_message.assert_called_once_with(mock_update, mock_context, "Mocked transcription text")
    handlers.os.remove.assert_called_once_with(expected_filename)


async def test_clear_context(mock_update, mock_context, mocker):
    mock_clear_history = mocker.patch('handlers.clear_chat_history')
    mocker.patch('handlers.get_user_model', return_value="Llama 3.3 70B 8K (groq)")
    mock_update.message.text = "Очистить контекст"
    await handle_message(mock_update, mock_context)
    mock_clear_history.assert_called_once_with(12345)
    mock_update.message.reply_text.assert_called_once_with(
        '<b>История чата очищена.</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

async def test_change_model_show_options(mock_update, mock_context, mocker):
    mock_update.message.text = "Сменить модель"
    await handle_message(mock_update, mock_context)
    mock_update.message.reply_text.assert_called_once_with(
        'Выберите модель:',
        reply_markup=get_model_keyboard()
    )

async def test_change_model_select_valid(mock_update, mock_context, mocker):
    selected_model = "Mistral Large 128K"
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model
    await handle_message(mock_update, mock_context)
    mock_update_model_db.assert_called_once_with(12345, selected_model)
    assert mock_context.user_data['model'] == selected_model
    mock_update.message.reply_text.assert_called_once_with(
        f'Модель изменена на <b>{selected_model}</b>',
        parse_mode=ParseMode.HTML,
        reply_markup=get_main_keyboard()
    )

async def test_change_model_direct_call(mock_update, mock_context, mocker):
    selected_model = "Llama 3.3 70B 8K (groq)"
    mock_update_model_db = mocker.patch('handlers.update_user_model')
    mock_update.message.text = selected_model
    await change_model(mock_update, mock_context)
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
    mock_context.args = ["invalid_id"]
    await add_user(mock_update, mock_context)
    mock_add_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя и роль (ADMIN или USER). Пример: /add_user 123456789 USER")

async def test_admin_add_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER)
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
    mock_context.args = []
    await remove_user(mock_update, mock_context)
    mock_remove_db.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with("Пожалуйста, укажите корректный ID пользователя. Пример: /remove_user 123456789")

async def test_admin_remove_user_not_admin(mock_update, mock_context, mocker):
    mocker.patch('handlers.get_user_role', return_value=UserRole.USER)
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
    mock_context.user_data['editing_prompt'] = False
    await handle_message(mock_update, mock_context)
    mock_update.message.reply_text.assert_called_once_with(
        'Выберите действие: (Или начните диалог)',
        reply_markup=get_main_keyboard()
    )

async def test_handle_unsupported_document(mock_update, mock_context, mocker):
    mock_update.message.document = MagicMock()
    mock_update.message.document.file_name = "test.zip"
    mock_update.message.text = None
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)
    await handle_message(mock_update, mock_context)
    mock_update.message.reply_text.assert_called_once_with("Данный файл не поддерживается.")
    mock_process_message.assert_not_called()

async def test_handle_voice_transcription_error(mock_update, mock_context, mocker):
    mock_voice = MagicMock(spec=Voice)
    mock_file_instance = await telegram.Voice.get_file()
    mock_voice.get_file = AsyncMock(return_value=mock_file_instance)
    mock_update.message.voice = mock_voice
    mock_update.message.text = None
    error_message = "Ошибка Gemini API: Квота"
    mocker.patch('handlers.audio_to_text', new_callable=AsyncMock, side_effect=Exception(error_message))
    mock_process_message = mocker.patch('handlers.process_message', new_callable=AsyncMock)

    await handle_voice(mock_update, mock_context)

    mock_process_message.assert_not_called()
    mock_update.message.reply_text.assert_called_once_with(
        f"Произошла ошибка при обработке голосового сообщения: {error_message}"
    )
    expected_filename = f"tempvoice_{mock_update.effective_user.id}.ogg"
    handlers.os.remove.assert_called_once_with(expected_filename)

    mock_voice.get_file.assert_called_once()
    mock_file_instance.download_as_bytearray.assert_called_once()


@pytest.fixture
def mock_gemini_dependencies(mocker):
    mocker.patch('handlers.gemini_client', new_callable=MagicMock)
    mock_gen_model_instance = MagicMock()
    mock_gen_model_instance.generate_content_async = AsyncMock()
    mocker.patch('handlers.gemini_client.GenerativeModel', return_value=mock_gen_model_instance)

    mock_upload = mocker.patch('handlers.genai.upload_file', return_value=MagicMock(name="default_mock_file", uri="default_uri"))
    mock_delete = mocker.patch('handlers.genai.delete_file')

    mock_sleep = mocker.patch('asyncio.sleep', new_callable=AsyncMock)

    mocker.patch('handlers.MODELS', {
        "Gemini 2.0 Flash": {"id": "gemini-2.0-flash", "provider": "gemini", "max_tokens": 1024},
    })

    return {
        "mock_gen_model": mock_gen_model_instance,
        "mock_upload_file": mock_upload,
        "mock_delete_file": mock_delete,
        "mock_sleep": mock_sleep
    }


@pytest.mark.asyncio
async def test_audio_to_text_success_no_retry(mock_gemini_dependencies, mocker): # Добавлен mocker
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload", uri="http://example.com/file")
    mock_upload_func.return_value = mock_uploaded_file_obj
    mock_gen_model.generate_content_async.return_value = MagicMock(text="Успешная транскрипция")

    result = await audio_to_text("fake_path.ogg", "audio/ogg")

    assert result == "Успешная транскрипция"

    # --- НАЧАЛО ИСПРАВЛЕНИЯ ---
    # Использовать assert_called_once_with для надежной проверки аргументов
    expected_prompt = "Сделай точную транскрипцию речи из этого аудио/видео файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это."

    # Использовать mock.ANY для сложных объектов, если точное значение не важно
    mock_gen_model.generate_content_async.assert_called_once_with(
        contents=[expected_prompt, mock_uploaded_file_obj],
        generation_config=mock.ANY, # Используем mock.ANY
        safety_settings={'HARASSMENT':'block_none',
                         'HATE_SPEECH':'block_none',
                         'SEXUALLY_EXPLICIT':'block_none'}
    )
    # --- КОНЕЦ ИСПРАВЛЕНИЯ ---

    # Проверить вызовы upload и delete отдельно
    mock_upload_func.assert_called_once_with(path="fake_path.ogg", mime_type="audio/ogg")
    mock_delete_func.assert_called_once_with(name="files/test_upload")
    mock_sleep.assert_not_called() # Убедиться, что повторных попыток не было

@pytest.mark.asyncio
async def test_audio_to_text_retry_on_upload_503(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload_retry", uri="http://example.com/file_retry")
    mock_upload_func.side_effect = [
        google.api_core.exceptions.ServiceUnavailable("503 Error on upload"),
        mock_uploaded_file_obj
    ]
    mock_gen_model.generate_content_async.return_value = MagicMock(text="Транскрипция после ретрая загрузки")

    result = await audio_to_text("retry_upload.ogg", "audio/ogg")

    assert result == "Транскрипция после ретрая загрузки"
    assert mock_upload_func.call_count == 2
    mock_upload_func.assert_has_calls([
        call(path="retry_upload.ogg", mime_type="audio/ogg"),
        call(path="retry_upload.ogg", mime_type="audio/ogg")
    ])
    mock_sleep.assert_called_once_with(RETRY_DELAY_SECONDS * 1)
    mock_gen_model.generate_content_async.assert_called_once()
    mock_delete_func.assert_called_once_with(name="files/test_upload_retry")

@pytest.mark.asyncio
async def test_audio_to_text_retry_on_generate_503(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload_gen_retry", uri="http://example.com/file_gen_retry")
    mock_upload_func.return_value = mock_uploaded_file_obj
    mock_gen_model.generate_content_async.side_effect = [
        google.api_core.exceptions.ServiceUnavailable("503 Error on generate"),
        MagicMock(text="Транскрипция после ретрая генерации")
    ]

    result = await audio_to_text("retry_generate.mp4", "video/mp4")

    assert result == "Транскрипция после ретрая генерации"
    mock_upload_func.assert_called_once_with(path="retry_generate.mp4", mime_type="video/mp4")
    assert mock_gen_model.generate_content_async.call_count == 2
    mock_sleep.assert_called_once_with(RETRY_DELAY_SECONDS * 1)
    mock_delete_func.assert_called_once_with(name="files/test_upload_gen_retry")

@pytest.mark.asyncio
async def test_audio_to_text_fail_after_max_retries_upload(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]

    mock_upload_func.side_effect = google.api_core.exceptions.ServiceUnavailable("503 Error always")

    with pytest.raises(Exception, match=f"Ошибка Gemini API: Сервис недоступен после {MAX_RETRIES} попыток"):
        await audio_to_text("fail_upload.ogg", "audio/ogg")

    assert mock_upload_func.call_count == MAX_RETRIES
    assert mock_sleep.call_count == MAX_RETRIES - 1
    expected_sleep_calls = [call(RETRY_DELAY_SECONDS * i) for i in range(1, MAX_RETRIES)]
    mock_sleep.assert_has_calls(expected_sleep_calls)
    mock_gen_model.generate_content_async.assert_not_called()
    mock_delete_func.assert_not_called()

@pytest.mark.asyncio
async def test_audio_to_text_fail_after_max_retries_generate(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload_fail_gen", uri="http://example.com/file_fail_gen")
    mock_upload_func.return_value = mock_uploaded_file_obj
    mock_gen_model.generate_content_async.side_effect = google.api_core.exceptions.ServiceUnavailable("503 Error always")

    with pytest.raises(Exception, match=f"Ошибка Gemini API: Сервис недоступен после {MAX_RETRIES} попыток"):
        await audio_to_text("fail_generate.ogg", "audio/ogg")

    mock_upload_func.assert_called_once()
    assert mock_gen_model.generate_content_async.call_count == MAX_RETRIES
    assert mock_sleep.call_count == MAX_RETRIES - 1
    expected_sleep_calls = [call(RETRY_DELAY_SECONDS * i) for i in range(1, MAX_RETRIES)]
    mock_sleep.assert_has_calls(expected_sleep_calls)
    mock_delete_func.assert_called_once_with(name="files/test_upload_fail_gen")

@pytest.mark.asyncio
async def test_audio_to_text_non_retryable_error_upload(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]

    mock_upload_func.side_effect = ValueError("Invalid file format")

    with pytest.raises(Exception, match="Ошибка Gemini API при транскрипции: Invalid file format"):
        await audio_to_text("invalid.txt", "text/plain")

    mock_upload_func.assert_called_once()
    mock_sleep.assert_not_called()
    mock_delete_func.assert_not_called()

@pytest.mark.asyncio
async def test_audio_to_text_non_retryable_error_generate(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_sleep = mock_gemini_dependencies["mock_sleep"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload_other_err", uri="http://example.com/file_other_err")
    mock_upload_func.return_value = mock_uploaded_file_obj
    mock_gen_model.generate_content_async.side_effect = google.api_core.exceptions.InvalidArgument("Bad request")

    expected_error_message = "Ошибка Gemini API при транскрипции: 400 Bad request"
    with pytest.raises(Exception, match=expected_error_message):
         await audio_to_text("other_error.ogg", "audio/ogg")

    mock_upload_func.assert_called_once()
    mock_gen_model.generate_content_async.assert_called_once()
    mock_sleep.assert_not_called()
    mock_delete_func.assert_called_once_with(name="files/test_upload_other_err")

@pytest.mark.asyncio
async def test_audio_to_text_handles_gemini_error_message(mock_gemini_dependencies):
    mock_upload_func = mock_gemini_dependencies["mock_upload_file"]
    mock_gen_model = mock_gemini_dependencies["mock_gen_model"]
    mock_delete_func = mock_gemini_dependencies["mock_delete_file"]

    mock_uploaded_file_obj = MagicMock(name="files/test_upload_no_speech", uri="http://example.com/file_no_speech")
    mock_upload_func.return_value = mock_uploaded_file_obj
    mock_gen_model.generate_content_async.return_value = MagicMock(text="В файле не обнаружено речи.")

    result = await audio_to_text("no_speech.ogg", "audio/ogg")

    assert result == "(Gemini): В файле не обнаружено речи."
    mock_upload_func.assert_called_once()
    mock_gen_model.generate_content_async.assert_called_once()
    mock_delete_func.assert_called_once_with(name="files/test_upload_no_speech")