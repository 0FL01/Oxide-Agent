import os
import json
import logging
import boto3
from botocore.config import Config
from botocore.exceptions import ClientError
from enum import Enum
from typing import Optional, List, Dict
from config import ADMIN_ID

logger = logging.getLogger(__name__)

class UserRole(Enum):
    ADMIN = "ADMIN"
    USER = "USER"

class R2Storage:
    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super(R2Storage, cls).__new__(cls)
            cls._instance._client = None
            cls._instance.bucket = None
        return cls._instance

    @property
    def client(self):
        if self._client is None:
            self._init_client()
        return self._client

    @client.setter
    def client(self, value):
        self._client = value

    @client.deleter
    def client(self):
        self._client = None

    def _init_client(self):
        endpoint_url = os.getenv('R2_ENDPOINT_URL')
        access_key = os.getenv('R2_ACCESS_KEY_ID')
        secret_key = os.getenv('R2_SECRET_ACCESS_KEY')
        self.bucket = os.getenv('R2_BUCKET_NAME')

        if not all([endpoint_url, access_key, secret_key, self.bucket]):
            logger.error("R2 configuration is missing some environment variables.")
        
        self._client = boto3.client(
            's3',
            endpoint_url=endpoint_url,
            aws_access_key_id=access_key,
            aws_secret_access_key=secret_key,
            config=Config(signature_version='s3v4')
        )

    def save_json(self, key: str, data: any):
        try:
            self.client.put_object(
                Bucket=self.bucket,
                Key=key,
                Body=json.dumps(data, ensure_ascii=False, indent=2),
                ContentType='application/json'
            )
        except Exception as e:
            logger.error(f"Error saving to R2 (key: {key}): {e}")
            raise

    def load_json(self, key: str, default: any = None) -> any:
        try:
            response = self.client.get_object(Bucket=self.bucket, Key=key)
            return json.loads(response['Body'].read().decode('utf-8'))
        except ClientError as e:
            if e.response['Error']['Code'] == "NoSuchKey":
                return default
            logger.error(f"Error loading from R2 (key: {key}): {e}")
            return default
        except Exception as e:
            logger.error(f"Error loading from R2 (key: {key}): {e}")
            return default


    def delete_object(self, key: str):
        try:
            self.client.delete_object(Bucket=self.bucket, Key=key)
        except ClientError as e:
            if e.response['Error']['Code'] == "NoSuchKey":
                pass
            else:
                logger.error(f"Error deleting from R2 (key: {key}): {e}")
        except Exception as e:
            logger.error(f"Error deleting from R2 (key: {key}): {e}")


# Paths
ALLOWED_USERS_KEY = "registry/allowed_users.json"
def user_config_key(user_id: int) -> str: return f"users/{user_id}/config.json"
def user_history_key(user_id: int) -> str: return f"users/{user_id}/history.json"

storage = R2Storage()

# --- Registry Functions ---

def _get_allowed_users_map() -> Dict[str, str]:
    return storage.load_json(ALLOWED_USERS_KEY, {})

def is_user_allowed(user_id: int) -> bool:
    if user_id == ADMIN_ID:
        return True
    return str(user_id) in _get_allowed_users_map()

def get_user_role(user_id: int) -> Optional[UserRole]:
    if user_id == ADMIN_ID:
        return UserRole.ADMIN
    role_str = _get_allowed_users_map().get(str(user_id))
    return UserRole(role_str) if role_str else None

def add_allowed_user(user_id: int, role: UserRole):
    users = _get_allowed_users_map()
    users[str(user_id)] = role.value
    storage.save_json(ALLOWED_USERS_KEY, users)

def remove_allowed_user(user_id: int):
    users = _get_allowed_users_map()
    uid_str = str(user_id)
    if uid_str in users:
        del users[uid_str]
        storage.save_json(ALLOWED_USERS_KEY, users)
        # Optionally clean up user data
        storage.delete_object(user_config_key(user_id))
        storage.delete_object(user_history_key(user_id))

def list_allowed_users(limit: int = 500) -> List[Dict]:
    users = _get_allowed_users_map()
    # Sort and limit as previously done in SQL
    sorted_ids = sorted([int(uid) for uid in users.keys()])[:limit]
    return [{"telegram_id": uid, "role": users[str(uid)]} for uid in sorted_ids]

def get_allowed_user(user_id: int) -> Optional[Dict]:
    role = get_user_role(user_id)
    return {"telegram_id": user_id, "role": role.value} if role else None

# --- User Config Functions ---

def _get_user_config(user_id: int) -> Dict:
    return storage.load_json(user_config_key(user_id), {})

def _update_user_config(user_id: int, updates: Dict):
    config = _get_user_config(user_id)
    config.update(updates)
    storage.save_json(user_config_key(user_id), config)

def update_user_prompt(telegram_id: int, system_prompt: str):
    _update_user_config(telegram_id, {"system_prompt": system_prompt})

def get_user_prompt(telegram_id: int) -> Optional[str]:
    return _get_user_config(telegram_id).get("system_prompt")

def update_user_model(telegram_id: int, model_name: str):
    _update_user_config(telegram_id, {"model_name": model_name})

def get_user_model(telegram_id: int) -> Optional[str]:
    return _get_user_config(telegram_id).get("model_name")

# --- History Functions ---

def save_message(telegram_id: int, role: str, content: str):
    history = storage.load_json(user_history_key(telegram_id), [])
    history.append({
        "role": role,
        "content": content
    })
    # Keep it reasonable, e.g., last 100 messages if needed, but here we just append
    storage.save_json(user_history_key(telegram_id), history)

def get_chat_history(telegram_id: int, limit: int = 10) -> List[Dict]:
    history = storage.load_json(user_history_key(telegram_id), [])
    return history[-limit:]

def clear_chat_history(telegram_id: int):
    storage.delete_object(user_history_key(telegram_id))

# --- Legacy/compatibility ---

def create_chat_history_table():
    pass # No-op for R2

def create_user_models_table():
    pass # No-op for R2

def check_postgres_connection():
    # Replace with R2 healthcheck
    try:
        storage.client.list_buckets()
        logger.info("Successfully connected to R2 storage.")
    except Exception as e:
        logger.error(f"R2 connectivity test failed: {e}")

def get_db_connection():
    # This shouldn't be called anymore, but let's provide a dummy for compatibility if needed
    # though it's better to remove all usages.
    return None