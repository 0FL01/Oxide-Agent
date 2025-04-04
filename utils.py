import html
import re
import os
from typing import List, Dict, Tuple
from enum import Enum
import logging
from typing import Union

logger = logging.getLogger(__name__)

def clean_html(text):
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

def format_text(text):
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