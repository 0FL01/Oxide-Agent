Проверил актуальные открытые чекпоинты и тут есть главный нюанс: **“GLM-4.7” — не один токенизатор**. У как минимум двух открытых веток разные словари и разные ID спецтокенов: у **`zai-org/GLM-4.7`** `vocab_size = 151552`, `pad_token_id = 151329`, `eos_token_id = [151329, 151336, 151338]`; у **`zai-org/GLM-4.7-Flash`** уже `vocab_size = 154880`, `pad_token_id = 154820`, `eos_token_id = [154820, 154827, 154829]`. У **`zai-org/GLM-5`** тоже `vocab_size = 154880`, а в FP8-конфиге видны `eos_token_id = [154820, 154827, 154829]` и `model_max_length = 202752`. То есть для Rust нельзя делать “один glm tokenizer на всё” — нужно привязываться к **конкретному repo/checkpoint**. ([Hugging Face][1])

Самый практичный вывод для интеграции: у свежих чекпоинтов есть **`tokenizer.json`**, а у GLM-5 `tokenizer_config.json` прямо указывает backend `"tokenizers"` и `tokenizer_class: "TokenizersBackend"`. Это хорошо для Rust: вместо попытки вручную повторять Python-класс, можно грузить **ровно тот `tokenizer.json`**, который лежит рядом с моделью, через библиотеку Hugging Face `tokenizers`. Официальная документация `tokenizers` и `docs.rs` как раз описывает загрузку токенизатора из `tokenizer.json` через `Tokenizer::from_file`. ([Hugging Face][2])

Вторая важная вещь: для **GLM-4.7-Flash / GLM-5** считать токены надо **не по “голому” user text**, а по уже отрендеренному chat prompt. В их `chat_template.jinja` диалог начинается с **`[gMASK]<sop>`**, затем идут роли `<|system|>`, `<|user|>`, `<|assistant|>`, `<|observation|>`, а tool calls и tool responses оформляются XML-подобными тегами вроде `<tool_call>`, `<arg_key>`, `<arg_value>`, `<tool_response>`. При `add_generation_prompt` шаблон дописывает `<|assistant|>` и затем либо `<think>`, либо `</think>` в зависимости от режима thinking. Если ты хочешь, чтобы токенкаунт и prompt layout совпадали с Python/HF, в Rust надо воспроизводить именно этот шаблон. ([Hugging Face][3])

У **GLM-4.7-Flash** в `tokenizer_config.json` явно видны ID и строки спецтокенов: `<|endoftext|>`, `[MASK]`, `[gMASK]`, `[sMASK]`, `<sop>`, `<eop>`, роли (`<|system|>`, `<|user|>`, `<|assistant|>`, `<|observation|>`), мультимодальные маркеры, а также `<think>`, `</think>`, `<tool_call>`, `</tool_call>`, `<tool_response>`, `</tool_response>`, `<arg_key>`, `</arg_key>`, `<arg_value>`, `</arg_value>`, `/nothink`. Это значит, что как минимум для Flash-линейки reasoning/tooling-маркеры являются частью токенизаторной схемы, а не просто внешним текстовым соглашением. ([Hugging Face][4])

Что я бы советовал сделать в Rust:

```rust
use tokenizers::Tokenizer;

fn load_glm_tokenizer(path: &str) -> anyhow::Result<Tokenizer> {
    let tok = Tokenizer::from_file(path)?;
    Ok(tok)
}

fn main() -> anyhow::Result<()> {
    let tok = load_glm_tokenizer("./tokenizer.json")?;

    let prompt = "[gMASK]<sop><|user|>Привет\n<|assistant|><think>";
    let enc = tok.encode(prompt, false)?;
    println!("tokens = {}", enc.len());
    println!("ids = {:?}", enc.get_ids());

    Ok(())
}
```

Ключевой момент здесь — `prompt` уже должен быть **отрендерен по шаблону модели**, а не просто содержать пользовательский текст. Это соответствует тому, как `tokenizers` работает как pipeline над уже подготовленной строкой, а не как чат-рендерер. ([Docs.rs][5])

Для **GLM-4.7-Flash / GLM-5** минимальный рендерер в Rust можно сделать так:

```rust
#[derive(Debug, Clone)]
enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
struct Message {
    role: Role,
    content: String,
}

fn render_glm47_flash_or_glm5(messages: &[Message], add_generation_prompt: bool, enable_thinking: bool) -> String {
    let mut s = String::from("[gMASK]<sop>");

    for m in messages {
        match m.role {
            Role::System => {
                s.push_str("<|system|>");
                s.push_str(&m.content);
            }
            Role::User => {
                s.push_str("<|user|>");
                s.push_str(&m.content);
            }
            Role::Assistant => {
                s.push_str("<|assistant|>");
                s.push_str(&m.content);
            }
            Role::Tool => {
                s.push_str("<|observation|>");
                s.push_str("<tool_response>");
                s.push_str(&m.content);
                s.push_str("</tool_response>");
            }
        }
    }

    if add_generation_prompt {
        s.push_str("<|assistant|>");
        if enable_thinking {
            s.push_str("<think>");
        } else {
            s.push_str("</think>");
        }
    }

    s
}
```

Это не покрывает весь Jinja-шаблон 1 в 1, но уже совпадает с его базовой логикой: префикс `[gMASK]<sop>`, role markers, observation/tool_response, generation prompt с `<think>`/`</think>`. Полная поддержка tool calling потребует ещё вывода `<tool_call>...<arg_key>...<arg_value>...`. ([Hugging Face][3])

Что выбрать тебе по сути:

* Если целишься в **`GLM-5`** или **`GLM-4.7-Flash`**, бери их **собственный `tokenizer.json`** и воспроизводи их chat template. У них очень близкая схема: vocab 154880, длинный контекст, `[gMASK]<sop>`-старт и reasoning/tool шаблон. ([Hugging Face][6])
* Если целишься в **`GLM-4.7`** без Flash, это другая ветка: другой vocab/id-space. Нельзя безопасно переиспользовать токенизатор от Flash/GLM-5. ([Hugging Face][1])
* Для батчинга учитывай, что у свежих конфигов pad идёт через `<|endoftext|>` и используется **left padding**. Это видно у GLM-4.7-Flash и GLM-5 tokenizer configs. ([Hugging Face][4])

Итог:
**Не пытайся “понять общий tokenizer GLM” абстрактно. Для Rust-аппа правильный путь — выбрать точный checkpoint (`GLM-4.7`, `GLM-4.7-Flash` или `GLM-5`), скачать его `tokenizer.json`, в коде воспроизвести его chat template, и только потом считать/кодировать токены.** Это даст совпадение с HF и уберёт большую часть несовместимостей.

Могу сразу собрать тебе готовый `glm_tokenizer.rs` с рендерингом prompt’а под `GLM-5 / GLM-4.7-Flash`.

[1]: https://huggingface.co/zai-org/GLM-4.7/blob/main/config.json "https://huggingface.co/zai-org/GLM-4.7/blob/main/config.json"
[2]: https://huggingface.co/zai-org/GLM-5/blob/main/.gitattributes ".gitattributes · zai-org/GLM-5 at main"
[3]: https://huggingface.co/zai-org/GLM-5-FP8/commit/0ae1a5b2cda14f9463c74f7d8350723dc8665466 "Add files using upload-large-folder tool · zai-org/GLM-5-FP8 at 0ae1a5b"
[4]: https://huggingface.co/zai-org/GLM-4.7-Flash/blob/main/tokenizer_config.json "https://huggingface.co/zai-org/GLM-4.7-Flash/blob/main/tokenizer_config.json"
[5]: https://docs.rs/tokenizers/?utm_source=chatgpt.com "tokenizers - Rust"
[6]: https://huggingface.co/zai-org/GLM-4.7-Flash/blob/75fce703a93da3705c4cf8d3caadbac1d1072c78/config.json "https://huggingface.co/zai-org/GLM-4.7-Flash/blob/75fce703a93da3705c4cf8d3caadbac1d1072c78/config.json"
