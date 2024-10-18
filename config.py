import os
from groq import AsyncGroq
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
from openai import OpenAI
from mistralai import Mistral
from together import Together
import base64
import docx
import openpyxl
import xlrd
import pandas as pd

load_dotenv()

TELEGRAM_TOKEN = os.getenv('TELEGRAM_TOKEN')
GROQ_API_KEY = os.getenv('GROQ_API_KEY')
OPENROUTER_API_KEY = os.getenv('OPENROUTER_API_KEY')
HYPERBOLIC_API_KEY = os.getenv('HYPERBOLIC_API_KEY')
TOGETHER_API_KEY = os.getenv('TOGETHER_API_KEY')
MISTRAL_API_KEY = os.getenv('MISTRAL_API_KEY')
groq_client = AsyncGroq(api_key=GROQ_API_KEY)
GITHUB_TOKEN = os.getenv('GITHUB_TOKEN')

together_client = Together(api_key=TOGETHER_API_KEY)


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
}

DEFAULT_MODEL = "Llama 3.1 70B 8K (groq)"

ADMIN_ID = int(os.getenv('ADMIN_ID'))

def encode_image(image_path):
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode('utf-8')

def process_file(file_path):
    file_extension = os.path.splitext(file_path)[1].lower()
    content = ""

    try:
        if file_extension in ['.docx', '.doc']:
            doc = docx.Document(file_path)
            content = "\n".join([paragraph.text for paragraph in doc.paragraphs])
        elif file_extension in ['.xlsx', '.xls']:
            if file_extension == '.xlsx':
                wb = openpyxl.load_workbook(file_path)
                sheet = wb.active
                content = "\n".join([", ".join([str(cell.value) for cell in row]) for row in sheet.iter_rows()])
            else:
                wb = xlrd.open_workbook(file_path)
                sheet = wb.sheet_by_index(0)
                content = "\n".join([", ".join([str(sheet.cell_value(row, col)) for col in range(sheet.ncols)]) for row in range(sheet.nrows)])
        elif file_extension == '.csv':
            df = pd.read_csv(file_path)
            content = df.to_string(index=False)
        else:
            content = "Unsupported file type"
    except Exception as e:
        content = f"Error processing file: {str(e)}"
    
    return content
