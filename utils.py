import html
import re
import os
import logging
from typing import List, Dict, Tuple, Union, Optional, Any

logger = logging.getLogger(__name__)

class SensitiveDataFilter(logging.Filter):
    def __init__(self) -> None:
        super().__init__()
        self.patterns = [
            (r'(https?:\/\/[^\/]+\/bot)([0-9]+:[A-Za-z0-9_-]+)(\/[^"\s]*)', r'\1[TELEGRAM_TOKEN]\3'),
            (r'([0-9]{8,10}:[A-Za-z0-9_-]{35})', '[TELEGRAM_TOKEN]'),
            (r'(bot[0-9]{8,10}:)[A-Za-z0-9_-]+', r'\1[TELEGRAM_TOKEN]')
        ]
        self.r2_patterns = [
             (r"R2_ACCESS_KEY_ID=[^\s&]+", "R2_ACCESS_KEY_ID=[MASKED]"),
             (r"R2_SECRET_ACCESS_KEY=[^\s&]+", "R2_SECRET_ACCESS_KEY=[MASKED]"),
             (r"'aws_access_key_id': '[^']*'", "'aws_access_key_id': '[MASKED]'"),
             (r"'aws_secret_access_key': '[^']*'", "'aws_secret_access_key': '[MASKED]'")
        ]

    def filter(self, record: logging.LogRecord) -> bool:
        if hasattr(record, 'msg'):
            if isinstance(record.msg, str):
                for pattern, replacement in self.patterns:
                    record.msg = re.sub(pattern, replacement, record.msg)
                for pattern, replacement in self.r2_patterns:
                     record.msg = re.sub(pattern, replacement, record.msg)

        if hasattr(record, 'args') and record.args:
            args_list = list(record.args)
            for i, arg in enumerate(args_list):
                if isinstance(arg, str):
                    for pattern, replacement in self.patterns:
                        args_list[i] = re.sub(pattern, replacement, args_list[i])
                    for pattern, replacement in self.r2_patterns:
                        args_list[i] = re.sub(pattern, replacement, args_list[i])
            record.args = tuple(args_list)
        return True

class TokenMaskingFormatter(logging.Formatter):
    def __init__(self, fmt: Optional[str] = None, datefmt: Optional[str] = None) -> None:
        super().__init__(fmt, datefmt)
        self.sensitive_filter = SensitiveDataFilter()

    def format(self, record: logging.LogRecord) -> str:
        self.sensitive_filter.filter(record)
        return super().format(record)

def clean_html(text: str) -> str:
    code_blocks = []

    def replace_code_block(match):
        code_blocks.append(match.group(0))
        return f"__CODE_BLOCK_{len(code_blocks)-1}__"

    text = re.sub(r'```[\s\S]*?```', replace_code_block, text)

    text = re.sub(r'<(?![/a-zA-Z])', '<', text)
    text = re.sub(r'(?<!>)>', '>', text)

    for i, block in enumerate(code_blocks):
        text = text.replace(f"__CODE_BLOCK_{i}__", block)

    return text

def format_text(text: str) -> str:
    text = clean_html(text)

    def code_block_replacer(match):
        code = match.group(2)
        language = match.group(1) or ''
        escaped_code = html.escape(code.strip())
        return f'<pre><code class="{language}">{escaped_code}</code></pre>'

    text = re.sub(r'```(\w+)?\n(.*?)```', code_block_replacer, text, flags=re.DOTALL)

    text = re.sub(r'^\* ', '• ', text, flags=re.MULTILINE)

    text = re.sub(r'\*\*(.*?)\*\*', r'<b>\1</b>', text)
    text = re.sub(r'\*(.*?)\*', r'<i>\1</i>', text)

    text = re.sub(r'`(.*?)`', lambda m: f'<code>{html.escape(m.group(1))}</code>', text)

    text = re.sub(r'\n{3,}', '\n\n', text)
    text = text.strip()

    return text


def split_long_message(message: str, max_length: int = 4000) -> list[str]:
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
        if line.startswith(code_fence):
            code_block = not code_block

        new_length = len(current_message) + len(line) + 1

        if new_length > max_length and current_message:
            if code_block:
                current_message += code_fence + '\n'
                code_block = False

            parts.append(current_message.rstrip())
            current_message = ""

            if line.startswith(code_fence):
                current_message = line + '\n'
            else:
                if code_block:
                    current_message = code_fence + '\n' + line + '\n'
                else:
                    current_message = line + '\n'
        else:
            current_message += line + '\n'

    if current_message:
        if code_block:
            current_message += code_fence + '\n'
        parts.append(current_message.rstrip())

    return parts