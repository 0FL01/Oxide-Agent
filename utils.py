import html
import re
import os
from typing import List, Dict, Tuple
from enum import Enum

def clean_html(text):
    """Remove improper HTML tags while preserving code blocks."""
    code_blocks = []
    
    def replace_code_block(match):
        code_blocks.append(match.group(0))
        return f"__CODE_BLOCK_{len(code_blocks)-1}__"
    
    text = re.sub(r'```[\s\S]*?```', replace_code_block, text)
    
    # Remove standalone angle brackets
    text = re.sub(r'<(?![/a-zA-Z])', '&lt;', text)
    text = re.sub(r'(?<!>)>', '&gt;', text)
    
    # Restore code blocks
    for i, block in enumerate(code_blocks):
        text = text.replace(f"__CODE_BLOCK_{i}__", block)
    
    return text

def format_text(text):
    """Format text with Telegram markdown."""
    text = clean_html(text)
    
    def code_block_replacer(match):
        code = match.group(2)
        language = match.group(1) or ''
        escaped_code = html.escape(code.strip())
        return f'```{language}\n{escaped_code}\n```'
    
    # Replace code blocks
    text = re.sub(r'```(\w+)?\n(.*?)```', code_block_replacer, text, flags=re.DOTALL)
    
    # Format lists
    text = re.sub(r'^\* ', '• ', text, flags=re.MULTILINE)
    
    # Format bold and italic
    text = re.sub(r'\*\*(.*?)\*\*', r'<b>\1</b>', text)
    text = re.sub(r'\*(.*?)\*', r'<i>\1</i>', text)
    
    # Format inline code
    text = re.sub(r'`(.*?)`', lambda m: f'<code>{html.escape(m.group(1))}</code>', text)
    
    # Clean up unnecessary whitespace
    text = re.sub(r'\n{3,}', '\n\n', text)
    text = text.strip()
    
    return text


def split_long_message(message: str, max_length: int = 4000) -> list[str]:
    """
    Split a long message into smaller chunks that fit within Telegram's message size limits.
    Preserves code blocks, markdown formatting, and natural text boundaries.

    Args:
        message (str): The message to split
        max_length (int): Maximum length of each chunk (default: 4000)

    Returns:
        list[str]: List of message chunks
    """
    if not message:
        return []

    if len(message) <= max_length:
        return [message]

    parts = []
    current_message = ""
    code_block = False
    code_fence = "```"

    lines = message.split('\n')

    for line in lines:
        # Check for code block boundaries
        if line.startswith(code_fence):
            code_block = not code_block

        # Calculate new length with potential new line
        new_length = len(current_message) + len(line) + 1  # +1 for newline

        if new_length > max_length and current_message:
            # If in code block, close it properly
            if code_block:
                current_message += code_fence + '\n'
                code_block = False

            parts.append(current_message.rstrip())
            current_message = ""

            # If we were in a code block, start a new one
            if line.startswith(code_fence):
                current_message = line + '\n'
            else:
                # Restart code block in new chunk if needed
                if code_block:
                    current_message = code_fence + '\n' + line + '\n'
                else:
                    current_message = line + '\n'
        else:
            current_message += line + '\n'

    # Add the last part if there's anything left
    if current_message:
        # Close any open code blocks
        if code_block:
            current_message += code_fence + '\n'
        parts.append(current_message.rstrip())

    return parts


class UserRole(Enum):
    ADMIN = "ADMIN"
    USER = "USER"

ALLOWED_USERS_FILE = "allowed_users.txt"

def load_allowed_users() -> Dict[int, UserRole]:
    if not os.path.exists(ALLOWED_USERS_FILE):
        return {}
    allowed_users = {}
    with open(ALLOWED_USERS_FILE, "r") as f:
        for line in f:
            parts = line.strip().split(',')
            if len(parts) == 2 and parts[0].isdigit():
                user_id = int(parts[0])
                role = UserRole(parts[1])
                allowed_users[user_id] = role
    return allowed_users

def save_allowed_users(users: Dict[int, UserRole]):
    with open(ALLOWED_USERS_FILE, "w") as f:
        for user_id, role in users.items():
            f.write(f"{user_id},{role.value}\n")

def is_user_allowed(user_id: int) -> bool:
    allowed_users = load_allowed_users()
    return user_id in allowed_users

def get_user_role(user_id: int) -> UserRole:
    allowed_users = load_allowed_users()
    return allowed_users.get(user_id, None)

def add_allowed_user(user_id: int, role: UserRole):
    allowed_users = load_allowed_users()
    allowed_users[user_id] = role
    save_allowed_users(allowed_users)

def remove_allowed_user(user_id: int):
    allowed_users = load_allowed_users()
    if user_id in allowed_users:
        del allowed_users[user_id]
        save_allowed_users(allowed_users)

user_auth_state: Dict[int, bool] = {}

def set_user_auth_state(user_id: int, state: bool):
    user_auth_state[user_id] = state

def get_user_auth_state(user_id: int) -> bool:
    return user_auth_state.get(user_id, False)

