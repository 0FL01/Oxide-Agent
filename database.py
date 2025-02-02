import os
import psycopg2
from psycopg2.extras import DictCursor
from enum import Enum
import logging
import socket

logger = logging.getLogger(__name__)

class UserRole(Enum):
    ADMIN = "ADMIN"
    USER = "USER"

def get_db_connection():
    connection_params = {
        'dbname': os.getenv('POSTGRES_DB'),
        'user': os.getenv('POSTGRES_USER'),
        'password': os.getenv('POSTGRES_PASSWORD'),
        'host': os.getenv('POSTGRES_HOST', '127.0.0.1'),
        'port': os.getenv('POSTGRES_PORT', '5432')
    }
    
    logger.info(f"Attempting to connect to database with params: {connection_params}")
    
    try:
        conn = psycopg2.connect(**connection_params)
        logger.info("Successfully connected to database")
        return conn
    except psycopg2.Error as e:
        logger.error(f"Failed to connect to database: {e}")
        logger.error(f"Connection error details: {e.diag.message_detail if hasattr(e, 'diag') else 'No details'}")
        raise

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

def check_postgres_connection():
    host = os.getenv('POSTGRES_HOST', '127.0.0.1')
    port = int(os.getenv('POSTGRES_PORT', '5432'))
    
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(5)
        result = sock.connect_ex((host, port))
        
        if result == 0:
            logger.info(f"Port {port} is open on host {host}")
        else:
            logger.error(f"Port {port} is closed on host {host}")
            
        sock.close()
    except Exception as e:
        logger.error(f"Network connectivity test failed: {e}")

def create_chat_history_table():
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute("""
                    CREATE TABLE IF NOT EXISTS chat_history (
                        id SERIAL PRIMARY KEY,
                        telegram_id BIGINT NOT NULL,
                        role VARCHAR(50) NOT NULL,
                        content TEXT NOT NULL,
                        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                        FOREIGN KEY (telegram_id) REFERENCES allowed_users(telegram_id) ON DELETE CASCADE
                    );
                    CREATE INDEX IF NOT EXISTS idx_telegram_id_created_at ON chat_history(telegram_id, created_at);
                """)
                conn.commit()
    except Exception as e:
        logger.error(f"Error creating chat_history table: {e}")
        raise

def save_message(telegram_id: int, role: str, content: str):
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    "INSERT INTO chat_history (telegram_id, role, content) VALUES (%s, %s, %s)",
                    (telegram_id, role, content)
                )
                conn.commit()
    except Exception as e:
        logger.error(f"Error saving message: {e}")
        raise

def get_chat_history(telegram_id: int, limit: int = 10) -> list:
    try:
        with get_db_connection() as conn:
            with conn.cursor(cursor_factory=DictCursor) as cur:
                cur.execute(
                    """
                    SELECT role, content 
                    FROM chat_history 
                    WHERE telegram_id = %s 
                    ORDER BY created_at DESC 
                    LIMIT %s
                    """,
                    (telegram_id, limit)
                )
                messages = cur.fetchall()
                return [{"role": msg["role"], "content": msg["content"]} for msg in messages][::-1]
    except Exception as e:
        logger.error(f"Error getting chat history: {e}")
        return []

def clear_chat_history(telegram_id: int):
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute("DELETE FROM chat_history WHERE telegram_id = %s", (telegram_id,))
                conn.commit()
    except Exception as e:
        logger.error(f"Error clearing chat history: {e}")
        raise

def update_user_prompt(telegram_id: int, system_prompt: str):
    try:
        with get_db_connection() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """
                    INSERT INTO user_prompts (telegram_id, system_prompt)
                    VALUES (%s, %s)
                    ON CONFLICT (telegram_id) DO UPDATE
                    SET system_prompt = EXCLUDED.system_prompt,
                        updated_at = CURRENT_TIMESTAMP
                    """,
                    (telegram_id, system_prompt)
                )
                conn.commit()
    except Exception as e:
        logger.error(f"Ошибка обновления пользовательского промпта для {telegram_id}: {e}")
        raise 