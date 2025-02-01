import os
import psycopg2
from psycopg2.extras import DictCursor
from enum import Enum
import logging

logger = logging.getLogger(__name__)

class UserRole(Enum):
    ADMIN = "ADMIN"
    USER = "USER"

def get_db_connection():
    return psycopg2.connect(
        dbname=os.getenv('POSTGRES_DB'),
        user=os.getenv('POSTGRES_USER'),
        password=os.getenv('POSTGRES_PASSWORD'),
        host=os.getenv('POSTGRES_HOST'),
        port=os.getenv('POSTGRES_PORT', '5432')
    )

def is_user_allowed(user_id: int) -> bool:
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute("SELECT EXISTS(SELECT 1 FROM allowed_users WHERE telegram_id = %s)", (user_id,))
                return cur.fetchone()[0]
    except Exception as e:
        logger.error(f"Database error in is_user_allowed: {e}")
        return False

def get_user_role(user_id: int) -> UserRole:
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute("SELECT role FROM allowed_users WHERE telegram_id = %s", (user_id,))
                result = cur.fetchone()
                return UserRole(result[0]) if result else None
    except Exception as e:
        logger.error(f"Database error in get_user_role: {e}")
        return None

def add_allowed_user(user_id: int, role: UserRole):
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    "INSERT INTO allowed_users (telegram_id, role) VALUES (%s, %s) ON CONFLICT (telegram_id) DO UPDATE SET role = %s",
                    (user_id, role.value, role.value)
                )
                conn.commit()
    except Exception as e:
        logger.error(f"Database error in add_allowed_user: {e}")
        raise

def remove_allowed_user(user_id: int):
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute("DELETE FROM allowed_users WHERE telegram_id = %s", (user_id,))
                conn.commit()
    except Exception as e:
        logger.error(f"Database error in remove_allowed_user: {e}")
        raise 