import pytest
from utils import clean_html, format_text, split_long_message

def test_clean_html():
    # Test basic HTML cleaning
    assert clean_html("<script>alert('xss')</script>") == "<script>alert('xss')</script>"
    
    # Test code block preservation
    input_text = "Before ```python\nprint('hello')\n``` After"
    assert clean_html(input_text) == input_text
    
    # Test angle bracket handling
    assert clean_html("1 < 2 &amp; 3 > 4") == "1 < 2 &amp; 3 > 4"

def test_format_text():
    # Test markdown to HTML conversion
    assert format_text("**bold**") == "<b>bold</b>"
    assert format_text("*italic*") == "<i>italic</i>"
    assert format_text("`code`") == "<code>code</code>"
    
    # Test code block formatting
    code_input = "```python\nprint('hello')\n```"
    expected = '<pre><code class="python">print(&#x27;hello&#x27;)</code></pre>'
    assert format_text(code_input) == expected
    
    # Test list formatting
    assert format_text("* item") == "• item"

def test_split_long_message():
    # Test short message
    assert split_long_message("short") == ["short"]
    
    # Test exact length
    long_msg = "a" * 4000
    assert split_long_message(long_msg) == [long_msg]
    
    # Test splitting with code blocks
    code_msg = "```\n" + "a" * 3000 + "\n```\n" + "b" * 2000
    parts = split_long_message(code_msg)
    assert len(parts) == 2
    assert "```" in parts[0]
    assert "```" in parts[1]
    
    # Test edge cases
    assert split_long_message("") == []
    assert split_long_message(None) == []