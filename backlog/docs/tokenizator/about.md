Проверил. Итог такой: для `tiktoken_rs` из вашего списка **по-настоящему рабочий вариант только GPT-5.4**, и то с важной оговоркой по имени модели. Для **GLM 4.7 / GLM 5 / Kimi K2.5 / MiniMax M2.5** это **не нативно совместимые** токенизаторы, то есть `tiktoken_rs` можно использовать разве что как очень грубую оценку, но не как источник точного числа токенов. ([Docs.rs][1])

По самому `tiktoken_rs`: в актуальном коде у него есть готовые encoding’и `O200kBase`, `O200kHarmony`, `Cl100kBase`, `P50kBase`, `R50kBase`, `P50kEdit`, `Gpt2`. В маппинге моделей он знает `gpt-5` и префикс `gpt-5-` как `O200kBase`, а `gpt-oss-*` как `O200kHarmony`. То есть библиотека ориентирована на OpenAI-семейство и его encoding’и, а не на токенизаторы сторонних вендоров. ([GitHub][2])

**GPT-5.4**: по сути совместимость есть, потому что для GPT-5-семейства в `tiktoken_rs` используется `O200kBase`. Но есть нюанс: в OpenAI docs модель называется именно `gpt-5.4`, а в `tiktoken_rs` я вижу маппинг только для `gpt-5` и `gpt-5-*`, не для `gpt-5.4` как отдельной строки. Значит, **автоопределение по строке `"gpt-5.4"` может не сработать**, но **ручной выбор `Tokenizer::O200kBase` — правильная стратегия**. Отдельно, `O200kHarmony` нужен для low-level harmony-формата у `gpt-oss`, а не как основной выбор для API-модели GPT-5.4. Это вывод по сопоставлению источников, а не прямая формулировка OpenAI. ([OpenAI Developers][3])

**GLM 4.7**: из коробки не совместим. У официального `GLM-4.7-Flash` свой `tokenizer_config.json`, где указан `tokenizer_class`/backend `TokenizersBackend`, свой набор спецтокенов (`<|endoftext|>`, `[MASK]`, `[gMASK]`, `<|system|>`, `<|user|>`, `<|assistant|>` и т.д.) и свой max length. Это явно не `cl100k_base` и не `o200k_base`, так что `tiktoken_rs` не даст точный подсчёт. ([Hugging Face][4])

**GLM 5**: та же история. В официальном `GLM-5-FP8` также используется `TokenizersBackend`, свой набор GLM-спецтокенов и свой tokenizer config; по структуре это тот же собственный GLM-токенизатор, а не OpenAI `tiktoken` encoding. Значит, для точных токенов `tiktoken_rs` не подходит. ([Hugging Face][5])

**Kimi K2.5**: тоже не совместим. У официальной модели на Hugging Face указан **Vocabulary Size = 160K**, а в `tokenizer_config.json` есть собственные спецтокены вроде токена `163602` для мультимодальных маркеров (`<|media_begin|>` / в соседнем коммите менялся вариант токена), то есть это свой токенизатор Moonshot, не один из стандартных encoding’ов `tiktoken_rs`. ([Hugging Face][6])

**MiniMax M2.5**: тоже не совместим из коробки. Формально в `tokenizer_config.json` стоит `GPT2Tokenizer`, но репозиторий модели поставляет собственные `tokenizer.json` и `vocab.json`, плюс у него есть added tokens начиная с `200000` и свои BOS/EOS/UNK. То есть это **не stock GPT-2 / r50k_base**, а кастомный GPT2-style tokenizer. Поэтому `tiktoken_rs::Gpt2` или `R50kBase` могут быть лишь приблизением, но не точным матчем. ([Hugging Face][7])

Практический вывод:

* **GPT-5.4** — да, используйте `o200k_base` **вручную**.
* **GLM 4.7 / GLM 5 / Kimi K2.5 / MiniMax M2.5** — **нет, не рассчитывайте на точность `tiktoken_rs`**.
* Для этих четырёх семейств лучше брать **родной tokenizer из Hugging Face / vendor SDK / API estimate endpoint**, а не `tiktoken_rs`. ([Docs.rs][1])

Если нужно, я могу сразу дать готовую шпаргалку на Rust: что именно вызывать для GPT-5.4 и чем заменить `tiktoken_rs` для GLM/Kimi/MiniMax.

[1]: https://docs.rs/crate/tiktoken-rs/latest "tiktoken-rs 0.9.1 - Docs.rs"
[2]: https://github.com/zurawiki/tiktoken-rs/blob/main/tiktoken-rs/src/tokenizer.rs "tiktoken-rs/tiktoken-rs/src/tokenizer.rs at main · zurawiki/tiktoken-rs · GitHub"
[3]: https://developers.openai.com/api/docs/models/gpt-5.4 "GPT-5.4 Model | OpenAI API"
[4]: https://huggingface.co/zai-org/GLM-4.7-Flash/blob/main/tokenizer_config.json "tokenizer_config.json · zai-org/GLM-4.7-Flash at main"
[5]: https://huggingface.co/zai-org/GLM-5-FP8/commit/0ae1a5b2cda14f9463c74f7d8350723dc8665466 "Add files using upload-large-folder tool · zai-org/GLM-5-FP8 at 0ae1a5b"
[6]: https://huggingface.co/moonshotai/Kimi-K2.5 "moonshotai/Kimi-K2.5 · Hugging Face"
[7]: https://huggingface.co/MiniMaxAI/MiniMax-M2.5/commit/8c436e563d876e41e089f8d24aa481e2f2978be4 "Add files using upload-large-folder tool · MiniMaxAI/MiniMax-M2.5 at 8c436e5"
