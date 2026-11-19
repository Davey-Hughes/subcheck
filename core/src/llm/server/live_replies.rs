//! Replies captured from llama-server, vLLM and Ollama's `/v1`, so the readers are pinned to what
//! the servers really send.

use std::path::Path;

use super::error_text;
use crate::llm::{Backend, Output, Usage, Used};
use crate::settings::Settings;
use crate::test_support::{FakeServer, Reply};

const OLLAMA: &str = include_str!("../../../tests/fixtures/llm/ollama.json");
const LLAMA_SERVER: &str = include_str!("../../../tests/fixtures/llm/llama-server.json");
const VLLM: &str = include_str!("../../../tests/fixtures/llm/vllm.json");
const OLLAMA_UNKNOWN: &str = include_str!("../../../tests/fixtures/llm/ollama-unknown-model.json");
const VLLM_UNKNOWN: &str = include_str!("../../../tests/fixtures/llm/vllm-unknown-model.json");
const OLLAMA_MODELS: &str = include_str!("../../../tests/fixtures/llm/ollama-models.json");
const LLAMA_SERVER_MODELS: &str =
    include_str!("../../../tests/fixtures/llm/llama-server-models.json");
const VLLM_MODELS: &str = include_str!("../../../tests/fixtures/llm/vllm-models.json");

#[test]
fn each_servers_completion_gives_its_reply_and_tokens() {
    for (server, body, input, cache_read) in [
        ("ollama", OLLAMA, 47, Some(0)),
        ("llama-server", LLAMA_SERVER, 47, Some(0)),
        ("vllm", VLLM, 48, None),
    ] {
        let answer = Output::OpenAi.read(body);
        assert_eq!(
            answer.reply.as_deref(),
            Ok("We should leave before it rains.\n"),
            "{server}"
        );
        assert_eq!(
            answer.used,
            Used::Reported(Usage {
                input_tokens: input,
                output_tokens: 8,
                cache_read_tokens: cache_read,
                cache_write_tokens: None,
                cost_usd: None,
            }),
            "{server}"
        );
    }
}

#[test]
fn an_unknown_model_is_reported_as_each_server_words_it() {
    assert_eq!(
        error_text(404, OLLAMA_UNKNOWN),
        "HTTP 404: model 'no-such-model' not found"
    );
    assert_eq!(
        error_text(404, VLLM_UNKNOWN),
        "HTTP 404: The model `no-such-model` does not exist. Valid aliases: google/gemma-4-12B-it-qat-w4a16-ct."
    );
}

#[test]
fn ready_settles_the_one_model_each_servers_list_names() {
    for (server, models, label) in [
        ("ollama", OLLAMA_MODELS, "local@gemma4:12b-it-qat"),
        ("llama-server", LLAMA_SERVER_MODELS, "local@gemma4"),
        (
            "vllm",
            VLLM_MODELS,
            "local@google/gemma-4-12B-it-qat-w4a16-ct",
        ),
    ] {
        let fake = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => Reply::ok(models),
            _ => Reply::status(404, ""),
        });
        let text = format!("[llm.local]\nurl = \"{}\"\n", fake.url);
        let settings = Settings::parse(Path::new("/c/config.toml"), &text).unwrap();
        let ready = Backend::named("local", &settings).unwrap().ready();
        assert_eq!(
            ready.map(|(b, _)| b.label),
            Ok(label.to_string()),
            "{server}"
        );
    }
}
