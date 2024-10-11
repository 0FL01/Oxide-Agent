import os
from groq import AsyncGroq
from dotenv import load_dotenv
from utils import load_allowed_users, save_allowed_users, is_user_allowed, add_allowed_user, remove_allowed_user, set_user_auth_state, get_user_auth_state
#from langchain.tools import DuckDuckGoSearchRun
from openai import OpenAI
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

groq_client = AsyncGroq(api_key=GROQ_API_KEY)

if OPENROUTER_API_KEY:
    openrouter_client = OpenAI(
        base_url="https://openrouter.ai/api/v1",
        api_key=OPENROUTER_API_KEY,
    )
else:
    print("Warning: OPENROUTER_API_KEY is not set in the environment variables.")
    openrouter_client = None

if HYPERBOLIC_API_KEY:
    hyperbolic_client = OpenAI(
        base_url="https://api.hyperbolic.xyz/v1",
        api_key=HYPERBOLIC_API_KEY,
    )
else:
    print("Warning: HYPERBOLIC_API_KEY is not set in the environment variables.")
    hyperbolic_client = None

chat_history = {}
user_settings = {}

MODELS = {
    #"Gemini 1.5 Flash 1M (or)": {"id": "google/gemini-flash-1.5", "max_tokens": 1000000, "provider": "openrouter", "vision": True},
    #"GPT 4o mini 128K": {"id": "openai/gpt-4o-mini", "max_tokens": 128000, "provider": "openrouter", "vision": True},
    #"Qwen2.5 72B 128K": {"id": "qwen/qwen-2.5-72b-instruct", "max_tokens": 128000, "provider": "openrouter"},
    "Gemma 2 9B 8K (groq)": {"id": "gemma2-9b-it", "max_tokens": 8192, "provider": "groq"},
    "Llama 3.1 70B 128K (or)": {"id": "meta-llama/llama-3.1-70b-instruct:free", "max_tokens": 128000, "provider": "openrouter"},
    #"Llama 3.1 405B 128K": {"id": "nousresearch/hermes-3-llama-3.1-405b:free", "max_tokens": 128000, "provider": "openrouter"}
    "Llama 3.1 405B 128K (or)": {"id": "meta-llama/llama-3.1-405b-instruct:free", "max_tokens": 128000, "provider": "openrouter"},
    "Llama 3.1 70B 8K (groq)": {"id": "llama-3.1-70b-versatile", "max_tokens": 8000, "provider": "groq"}
}

DEFAULT_MODEL = "Llama 3.1 70B 8K (groq)"

ADMIN_ID = int(os.getenv('ADMIN_ID'))

#search_tool = DuckDuckGoSearchRun()

def encode_image(image_path):
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode('utf-8')

# Функция для обработки различных типов файлов
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
