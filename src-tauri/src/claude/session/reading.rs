//! A transcript read whole from its bytes alone: everything the rest of Luna
//! reads of one, by the same functions, with no file, lock or cache between.
//! What the fuzz target drives (fuzz/, through `luna_lib::transcripts`), and
//! what the fixture tests hold to the file-based readings.

use super::*;

/// What Luna reads of a transcript, each part as the function named on it
/// reads it of a file whose tail is the whole of it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranscriptRead {
    /// Where a reader from the top stops: the end of the last complete line
    /// (`messages_from`).
    pub consumed: usize,
    /// The conversation without tool calls, and with them (`messages_from`).
    pub messages: Vec<crate::agents::Message>,
    pub with_tools: Vec<crate::agents::Message>,
    /// The opening prompt, as a title (`cached_first_prompt`).
    pub first_prompt: Option<String>,
    /// `/rename`'s title, and the one the CLI gave (`saved_title`).
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    /// The last answer's input-side tokens, and the model whose window they
    /// fill (`last_context`, which looks the window up).
    pub usage: Option<(f64, String)>,
    /// `last_model`.
    pub model: Option<String>,
    /// Some(what the last turn ended on, and when) once the text reaches back
    /// to the model's last words or to a prompt (`Ending::last_said`).
    pub last_said: Option<Option<(String, Option<String>)>>,
    /// Some(the API error line the last turn ended on, if it did) once the
    /// text reaches back to a turn (`api_error_of`).
    pub api_error: Option<Option<serde_json::Value>>,
    /// The tool call without a result, as (name, input) (`pending_tool`).
    pub pending_tool: Option<(String, serde_json::Value)>,
    /// `due_ms`.
    pub due_ms: Option<u64>,
}

/// Everything Luna reads of a transcript, from its bytes.
pub fn read_transcript(bytes: &[u8]) -> TranscriptRead {
    let mut said = Conversation::new(false);
    let mut with_tools = Conversation::new(true);
    let consumed = crate::pty::complete_lines_in(bytes, |line| {
        said.line(line);
        with_tools.line(line);
    });
    // As a tail read has it.
    let text = String::from_utf8_lossy(bytes);
    let titles = titles_in(&text);
    TranscriptRead {
        consumed,
        messages: said.messages,
        with_tools: with_tools.messages,
        first_prompt: first_prompt_in(crate::pty::text_lines(bytes)),
        custom_title: titles.custom,
        ai_title: titles.ai,
        usage: usage_in(&text),
        model: model_in(&text),
        last_said: last_said_in(&text),
        api_error: api_error_line(&text),
        pending_tool: pending_tool_in(&text),
        due_ms: due_in(&text),
    }
}
