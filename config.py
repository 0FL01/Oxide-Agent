import os
from groq import AsyncGroq
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from openai import OpenAI
from mistralai import Mistral
from together import Together
import base64
import json
import yaml
import xml.etree.ElementTree as ET
import docx
import openpyxl
import xlrd
import pandas as pd
from typing import Union
import logging

logger = logging.getLogger(__name__)

load_dotenv()

TELEGRAM_TOKEN = os.getenv('TELEGRAM_TOKEN')
GROQ_API_KEY = os.getenv('GROQ_API_KEY')
OPENROUTER_API_KEY = os.getenv('OPENROUTER_API_KEY')
HYPERBOLIC_API_KEY = os.getenv('HYPERBOLIC_API_KEY')
TOGETHER_API_KEY = os.getenv('TOGETHER_API_KEY')
MISTRAL_API_KEY = os.getenv('MISTRAL_API_KEY')
groq_client = AsyncGroq(api_key=GROQ_API_KEY)
GITHUB_TOKEN = os.getenv('GITHUB_TOKEN')


AZURE_ENDPOINT = "https://models.inference.ai.azure.com"

if GITHUB_TOKEN:
    azure_client = OpenAI(
        base_url=AZURE_ENDPOINT,
        api_key=GITHUB_TOKEN,
    )
else:
    print("Warning: GITHUB_TOKEN is not set in the environment variables.")
    azure_client = None


if OPENROUTER_API_KEY:
    openrouter_client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=OPENROUTER_API_KEY,
    )
else:
    print("Warning: OPENROUTER_API_KEY is not set in the environment variables.")
    openrouter_client = None

if TOGETHER_API_KEY:
    together_client = Together(
        base_url="https://api.together.xyz/v1",
        api_key=TOGETHER_API_KEY,
    )
else:
    print("Warning: TOGETHER_API_KEY is not set in the environment variables.")
    together_client = None

if HYPERBOLIC_API_KEY:
    hyperbolic_client = OpenAI(
        base_url="https://api.hyperbolic.xyz/v1",
        api_key=HYPERBOLIC_API_KEY,
    )
else:
    print("Warning: HYPERBOLIC_API_KEY is not set in the environment variables.")
    hyperbolic_client = None

if MISTRAL_API_KEY:
    mistral_client = Mistral(api_key=MISTRAL_API_KEY)
else:
    print("Warning: MISTRAL_API_KEY is not set in the environment variables.")
    mistral_client = None

chat_history = {}

MODELS = {
    "Mistral Large 128K": {"id": "mistral-large-2407", "max_tokens": 128000, "provider": "mistral"},
    "Llama 3.1 70B 128K (or)": {"id": "meta-llama/llama-3.1-70b-instruct:free", "max_tokens": 128000, "provider": "openrouter"},
    "Llama 3.1 405B 128K (or)": {"id": "nousresearch/hermes-3-llama-3.1-405b:free", "max_tokens": 128000, "provider": "openrouter"},
    "GPT-4o 8K (Azure)": {"id": "gpt-4o", "max_tokens": 8192, "provider": "azure", "vision": True},
    "GPT-4o-mini 16K (Azure)": {"id": "gpt-4o-mini", "max_tokens": 16192, "provider": "azure", "vision": True},
    "Llama 3.1 70B 8K (groq)": {"id": "llama-3.1-70b-versatile", "max_tokens": 8000, "provider": "groq"},
    "FLUX.1-schnell": {"id": "black-forest-labs/FLUX.1-schnell-Free", "provider": "together", "type": "image"}
}

DEFAULT_MODEL = "Llama 3.1 70B 8K (groq)"

def generate_image(prompt):
    if not TOGETHER_API_KEY:
        raise ValueError("TOGETHER_API_KEY is not set in the environment variables.")

    together_client = Together(api_key=TOGETHER_API_KEY)
    response = together_client.images.generate(
        prompt=prompt,
        model="black-forest-labs/FLUX.1-schnell-Free",
        width=1024,
        height=768,
        steps=1,
        n=1,
        response_format="b64_json"
    )
    return response.data[0].b64_json

ADMIN_ID = int(os.getenv('ADMIN_ID'))

def encode_image(image_path):
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode('utf-8')

def process_file(file_path: str) -> str:
    """
    Process various file formats and convert them to text.
    Supported formats: log, txt, json, xml, md, yaml, yml, doc, docx, csv, xls, xlsx

    Args:
        file_path (str): Path to the file to process

    Returns:
        str: Extracted text content from the file
    """
    file_extension = os.path.splitext(file_path)[1].lower()
    content = ""

    try:
        # Text-based files
        if file_extension in ['.txt', '.log', '.md']:
            with open(file_path, 'r', encoding='utf-8') as file:
                content = file.read()

        # JSON files
        elif file_extension == '.json':
            with open(file_path, 'r', encoding='utf-8') as file:
                json_data = json.load(file)
                content = json.dumps(json_data, indent=2, ensure_ascii=False)

        # YAML files
        elif file_extension in ['.yaml', '.yml']:
            with open(file_path, 'r', encoding='utf-8') as file:
                yaml_data = yaml.safe_load(file)
                content = yaml.dump(yaml_data, allow_unicode=True)

        # XML files
        elif file_extension == '.xml':
            tree = ET.parse(file_path)
            root = tree.getroot()

            def process_element(element, level=0):
                result = []
                indent = "  " * level
                result.append(f"{indent}{element.tag}:")

                if element.attrib:
                    result.append(f"{indent}  attributes: {element.attrib}")

                if element.text and element.text.strip():
                    result.append(f"{indent}  text: {element.text.strip()}")

                for child in element:
                    result.extend(process_element(child, level + 1))

                return result

            content = "\n".join(process_element(root))

        # Word documents
        elif file_extension in ['.docx', '.doc']:
            doc = docx.Document(file_path)
            paragraphs = []

            for paragraph in doc.paragraphs:
                if paragraph.text.strip():
                    paragraphs.append(paragraph.text)

            content = "\n\n".join(paragraphs)

        # Excel files
        elif file_extension == '.xlsx':
            wb = openpyxl.load_workbook(file_path)
            sheets_data = []

            for sheet_name in wb.sheetnames:
                sheet = wb[sheet_name]
                df = pd.DataFrame(sheet.values)
                if not df.empty:
                    sheets_data.append(f"Sheet: {sheet_name}\n{df.to_string(index=False)}")

            content = "\n\n".join(sheets_data)

        elif file_extension == '.xls':
            wb = xlrd.open_workbook(file_path)
            sheets_data = []

            for sheet_idx in range(wb.nsheets):
                sheet = wb.sheet_by_index(sheet_idx)
                data = []
                for row in range(sheet.nrows):
                    row_data = [str(sheet.cell_value(row, col)) for col in range(sheet.ncols)]
                    data.append("\t".join(row_data))
                sheets_data.append(f"Sheet: {sheet.name}\n{chr(10).join(data)}")

            content = "\n\n".join(sheets_data)

        # CSV files
        elif file_extension == '.csv':
            df = pd.read_csv(file_path)
            content = df.to_string(index=False)

        else:
            content = f"Unsupported file type: {file_extension}"

        # Add file metadata
        file_size = os.path.getsize(file_path) / 1024  # Size in KB
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
        return error_msg



