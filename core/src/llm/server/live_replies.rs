//! Replies captured from llama-server, vLLM and Ollama, so the readers are pinned to what the
//! servers really send.

use std::path::Path;

use super::error_text;
use crate::llm::{Backend, Context, Output, TIMEOUT, TooBig, Usage, Used};
use crate::settings::Settings;
use crate::test_support::{FakeServer, Reply, models_reply};

const OLLAMA: &str = include_str!("../../../tests/fixtures/llm/ollama.json");
const LLAMA_SERVER: &str = include_str!("../../../tests/fixtures/llm/llama-server.json");
const VLLM: &str = include_str!("../../../tests/fixtures/llm/vllm.json");
const OLLAMA_UNKNOWN: &str = include_str!("../../../tests/fixtures/llm/ollama-unknown-model.json");
const VLLM_UNKNOWN: &str = include_str!("../../../tests/fixtures/llm/vllm-unknown-model.json");
const OLLAMA_MODELS: &str = include_str!("../../../tests/fixtures/llm/ollama-models.json");
const LLAMA_SERVER_MODELS: &str =
    include_str!("../../../tests/fixtures/llm/llama-server-models.json");
const VLLM_MODELS: &str = include_str!("../../../tests/fixtures/llm/vllm-models.json");
const OLLAMA_PS: &str = include_str!("../../../tests/fixtures/llm/ollama-ps.json");
const LLAMA_SERVER_OVERFLOW: &str =
    include_str!("../../../tests/fixtures/llm/llama-server-overflow.json");
const VLLM_OVERFLOW: &str = include_str!("../../../tests/fixtures/llm/vllm-overflow.json");
const OLLAMA_OVERFLOW: &str = include_str!("../../../tests/fixtures/llm/ollama-overflow.json");
const LLAMA_SERVER_SHARED_CACHE: &str =
    include_str!("../../../tests/fixtures/llm/llama-server-shared-cache.json");

/// The backend of an entry `local` at `url` that names no model, readied.
fn readied(url: &str) -> Backend {
    let text = format!("[llm.local]\nurl = \"{url}\"\n");
    let settings = Settings::parse(Path::new("/c/config.toml"), &text).unwrap();
    Backend::named("local", &settings)
        .unwrap()
        .ready()
        .unwrap()
        .0
}

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

#[test]
fn ready_reads_the_context_each_servers_list_gives() {
    for (models, context) in [(LLAMA_SERVER_MODELS, 16384), (VLLM_MODELS, 16384)] {
        let fake = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => Reply::ok(models),
            _ => Reply::status(404, ""),
        });
        assert_eq!(readied(&fake.url).context(), Some(Context::Tokens(context)));
        assert!(
            fake.requests().iter().all(|r| r.path == "/v1/models"),
            "no /api/ps when /models says"
        );
    }
}

#[test]
fn ollama_gives_its_context_in_api_ps_once_the_model_is_loaded() {
    for (ps, context) in [
        (Reply::ok(OLLAMA_PS), Context::Tokens(16384)),
        (Reply::ok(r#"{"models":[]}"#), Context::NotLoaded),
        (Reply::status(404, "404 page not found"), Context::Unknown),
        (Reply::ok("<html>"), Context::Unknown),
    ] {
        let fake = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => Reply::ok(OLLAMA_MODELS),
            "/api/ps" => ps.clone(),
            _ => Reply::status(404, ""),
        });
        assert_eq!(readied(&fake.url).context(), Some(context));
        assert!(fake.requests().iter().all(|r| r.method == "GET"));
    }
}

#[test]
fn each_servers_answer_to_an_over_long_prompt_is_too_big_for_the_prompt() {
    for (server, status, body) in [
        ("llama-server", 400, LLAMA_SERVER_OVERFLOW),
        ("vllm", 400, VLLM_OVERFLOW),
        // As Ollama's native /api/chat words it.
        ("ollama", 400, OLLAMA_OVERFLOW),
        // llama-server, when the calls in flight outgrow the context they share.
        ("llama-server shared", 500, LLAMA_SERVER_SHARED_CACHE),
    ] {
        let fake = FakeServer::start(move |r| match r.path.as_str() {
            "/v1/models" => models_reply(&["m"]),
            "/v1/chat/completions" => Reply::status(status, body),
            _ => Reply::status(404, ""),
        });
        let a = readied(&fake.url).call("P", TIMEOUT, None);
        assert_eq!(
            (a.too_big, a.reply.is_err(), a.used),
            (Some(TooBig::Prompt), true, Used::Unreported),
            "{server}"
        );
    }
}
