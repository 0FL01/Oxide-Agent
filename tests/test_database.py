import pytest
import json
from unittest.mock import MagicMock, patch
from botocore.exceptions import ClientError
from database import R2Storage, UserRole, add_allowed_user, is_user_allowed, get_user_role, save_message, get_chat_history


@pytest.fixture
def mock_storage():
    with patch('database.storage.client') as mock_client:
        yield mock_client

def test_r2_save_json(mock_storage):
    storage = R2Storage()
    data = {"test": "data"}
    storage.save_json("test.json", data)
    
    mock_storage.put_object.assert_called_once()
    args, kwargs = mock_storage.put_object.call_args
    assert kwargs['Key'] == "test.json"
    assert json.loads(kwargs['Body']) == data

def test_r2_load_json_exists(mock_storage):
    storage = R2Storage()
    data = {"test": "data"}
    
    # Mock response
    mock_response = {
        'Body': MagicMock()
    }
    mock_response['Body'].read.return_value = json.dumps(data).encode('utf-8')
    mock_storage.get_object.return_value = mock_response
    
    result = storage.load_json("test.json")
    assert result == data

def test_r2_load_json_missing(mock_storage):
    storage = R2Storage()
    error_response = {'Error': {'Code': 'NoSuchKey'}}
    mock_storage.get_object.side_effect = ClientError(error_response, "GetObject")
    
    result = storage.load_json("missing.json", default={"default": True})
    assert result == {"default": True}


def test_add_allowed_user(mock_storage):
    # Mock sequence: load empty, then save
    mock_storage.get_object.side_effect = Exception("Not found") # Simplification for default empty
    
    with patch('database.storage.load_json', return_value={}):
        add_allowed_user(12345, UserRole.ADMIN)
        
    mock_storage.put_object.assert_called_once()
    _, kwargs = mock_storage.put_object.call_args
    assert json.loads(kwargs['Body']) == {"12345": "ADMIN"}

def test_is_user_allowed(mock_storage):
    with patch('database.storage.load_json', return_value={"12345": "USER"}), \
         patch('database.ADMIN_ID', 99999):
        assert is_user_allowed(12345) is True
        assert is_user_allowed(99999) is True
        assert is_user_allowed(888) is False

def test_get_user_role_admin_id():
    with patch('database.storage.load_json', return_value={"12345": "USER"}), \
         patch('database.ADMIN_ID', 99999):
        assert get_user_role(99999) == UserRole.ADMIN
        assert get_user_role(12345) == UserRole.USER
        assert get_user_role(888) is None

def test_save_and_get_history(mock_storage):
    history_key = "users/12345/history.json"
    existing_history = [{"role": "user", "content": "hello"}]
    
    with patch('database.storage.load_json', return_value=existing_history):
        save_message(12345, "assistant", "hi")
    
    # Check save
    mock_storage.put_object.assert_called_once()
    _, kwargs = mock_storage.put_object.call_args
    assert kwargs['Key'] == history_key
    saved_data = json.loads(kwargs['Body'])
    assert len(saved_data) == 2
    assert saved_data[1]["content"] == "hi"

    # Check get
    with patch('database.storage.load_json', return_value=saved_data):
        history = get_chat_history(12345, limit=1)
        assert len(history) == 1
        assert history[0]["content"] == "hi"
