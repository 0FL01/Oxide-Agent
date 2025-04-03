import html
import re
import os
from typing import List, Dict, Tuple
from enum import Enum
import xml.etree.ElementTree as ET
import docx
import openpyxl
import xlrd
import pandas as pd
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


def process_file(file_path: str, max_size: int = 1 * 1024 * 1024) -> str:
    if os.path.getsize(file_path) > max_size:
        raise ValueError(f"Файл слишком большой. Максимальный размер: {max_size/1024/1024}MB")

    file_extension = os.path.splitext(file_path)[1].lower()
    content = ""

    try:
        if file_extension in ['.txt', '.log', '.md']:
            with open(file_path, 'r', encoding='utf-8') as file:
                content = file.read()

        elif file_extension == '.xml':
            try:
                tree = ET.parse(file_path)
                root = tree.getroot()

                def process_element(element, level=0):
                    result = []
                    indent = "  " * level
                    attrib_str = ', '.join([f"{k}='{v}'" for k, v in element.attrib.items()])
                    tag_info = f"{element.tag}"
                    if attrib_str:
                        tag_info += f" ({attrib_str})"
                    result.append(f"{indent}{tag_info}")

                    if element.text and element.text.strip():
                        result.append(f"{indent}  {element.text.strip()}")

                    for child in element:
                        result.extend(process_element(child, level + 1))

                    return result

                content = "\n".join(process_element(root))
            except ET.ParseError as e:
                raise ValueError(f"Некорректный XML файл: {str(e)}")

        elif file_extension in ['.docx', '.doc']:
            try:
                doc = docx.Document(file_path)
                paragraphs = []

                for paragraph in doc.paragraphs:
                    if paragraph.text.strip():
                        style = paragraph.style.name if paragraph.style else "Normal"
                        paragraphs.append(f"[{style}] {paragraph.text}")

                content = "\n\n".join(paragraphs)
            except Exception as e:
                raise ValueError(f"Ошибка при обработке документа Word: {str(e)}")

        elif file_extension in ['.xlsx', '.xls']:
            try:
                if file_extension == '.xlsx':
                    df = pd.read_excel(file_path, engine='openpyxl')
                else:
                    df = pd.read_excel(file_path, engine='xlrd')

                content = (
                    f"Columns: {', '.join(df.columns)}\n"
                    f"Rows: {len(df)}\n\n"
                    f"{df.to_string(index=True, max_rows=1000)}"
                )
            except Exception as e:
                raise ValueError(f"Ошибка при обработке Excel файла: {str(e)}")

        elif file_extension == '.csv':
            try:
                df = pd.read_csv(file_path)
                content = (
                    f"Columns: {', '.join(df.columns)}\n"
                    f"Rows: {len(df)}\n\n"
                    f"{df.to_string(index=True, max_rows=1000)}"
                )
            except Exception as e:
                raise ValueError(f"Ошибка при обработке CSV файла: {str(e)}")

        else:
            content = f"Unsupported file type: {file_extension}"

        file_size = os.path.getsize(file_path) / 1024
        file_name = os.path.basename(file_path)
        metadata = (
            f"File Information:\n"
            f"Name: {file_name}\n"
            f"Type: {file_extension}\n"
            f"Size: {file_size:.2f} KB\n"
            f"---\n\n"
        )

        return metadata + content

    except Exception as e:
        error_msg = f"Error processing file {file_path}: {str(e)}"
        logger.error(error_msg)
        raise ValueError(error_msg)