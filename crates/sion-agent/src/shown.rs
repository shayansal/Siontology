//! A stand-in model that answers with what it was sent: the system prompt,
//! the tools with their descriptions and schemas, the conversation, and
//! how large each is.
//!
//! Before a model that costs money is asked, this shows what it would be
//! asked: put in a run's place of a model ([`Shown`]), it goes through all
//! that assembles a request (the session's prompt, what the agent knows of
//! itself, the toolbox a run's flags build, the tools a large toolbox
//! defers) and sends nothing to anyone. Its answer ends the run.

use crate::model::{Block, Message, Model, ModelError, Request, Response, Role, Stop, Usage};

/// The name [`Shown`] answers by.
pub const SHOWN: &str = "context";

/// A stand-in model that answers each request with a rendering of it
/// ([`render`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Whether each tool's description and schema are shown whole, rather
    /// than a line for each.
    pub full: bool,
}

impl Model for Shown {
    fn id(&self) -> &str {
        SHOWN
    }

    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        Ok(Response {
            content: vec![Block::text(render(request, self.full))],
            stop: Stop::EndTurn,
            usage: Usage::default(),
            model: SHOWN.into(),
            forgot_reasoning: false,
        })
    }
}

/// About how many tokens `chars` characters are, as a run counts them
/// against its limits: a token for every three, which counts English text
/// high.
pub fn tokens(chars: usize) -> usize {
    chars.div_ceil(3)
}

/// A number with its thousands apart: `12,345`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn sized(chars: usize) -> String {
    format!(
        "{} characters, about {} tokens",
        grouped(chars),
        grouped(tokens(chars))
    )
}

/// The first sentence of `text`, or its first line, at most `most`
/// characters.
fn first_words(text: &str, most: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let sentence = match line.find(". ") {
        Some(end) => &line[..=end],
        None => line,
    };
    if sentence.chars().count() <= most {
        return sentence.to_string();
    }
    let cut: String = sentence.chars().take(most.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// What one message holds, in a line.
fn message_line(message: &Message) -> String {
    let who = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    let mut parts = Vec::new();
    for block in &message.content {
        parts.push(match block {
            Block::Text { text } => format!("text ({})", grouped(text.len())),
            Block::ToolUse { name, .. } => format!("call {name}"),
            Block::ToolResult {
                content, is_error, ..
            } => format!(
                "{} ({})",
                if *is_error { "error" } else { "result" },
                grouped(content.len())
            ),
            Block::Image { .. } => "picture".to_string(),
            Block::Reasoning { .. } => "reasoning".to_string(),
        });
    }
    format!("  {who}: {}", parts.join(", "))
}

/// `request` as a person reads it: the system prompt whole; each tool in a
/// line (name, the sizes of its description and its schema, what it says
/// first), or whole if `full`; the conversation's messages in a line each;
/// and how large it all is, in characters and about in tokens.
pub fn render(request: &Request, full: bool) -> String {
    let mut out = String::new();
    out.push_str(
        "What a model is sent for this run (nothing was sent to a provider; sizes in tokens \
         are estimates, a token for every three characters).\n\n",
    );
    let system = request.system.len();
    out.push_str(&format!("System prompt: {}\n", sized(system)));
    out.push_str("--------\n");
    out.push_str(request.system.trim_end());
    out.push_str("\n--------\n\n");

    let size = |spec: &crate::model::ToolSpec| {
        (
            spec.description.len(),
            spec.input_schema.to_string().len() + spec.name.len(),
        )
    };
    let tools: usize = request
        .tools
        .iter()
        .map(|spec| {
            let (description, schema) = size(spec);
            description + schema
        })
        .sum();
    out.push_str(&format!(
        "Tools: {}, {}\n",
        request.tools.len(),
        sized(tools)
    ));
    let widest = request
        .tools
        .iter()
        .map(|spec| spec.name.len())
        .max()
        .unwrap_or(0);
    for spec in &request.tools {
        let (description, schema) = size(spec);
        if full {
            out.push_str(&format!(
                "\n{} (description {}, schema {})\n{}\n{}\n",
                spec.name,
                grouped(description),
                grouped(schema),
                spec.description.trim_end(),
                serde_json::to_string_pretty(&spec.input_schema).unwrap_or_default()
            ));
        } else {
            out.push_str(&format!(
                "  {:<widest$}  {:>6} + {:>6}  {}\n",
                spec.name,
                grouped(description),
                grouped(schema),
                first_words(&spec.description, 90)
            ));
        }
    }
    if !full && !request.tools.is_empty() {
        out.push_str(
            "  (each: the characters of its description + of its schema; all of each with \
             context:full)\n",
        );
    }

    let conversation: usize = request.messages.iter().map(Message::chars).sum();
    out.push_str(&format!(
        "\nConversation: {} message{}, {}\n",
        request.messages.len(),
        if request.messages.len() == 1 { "" } else { "s" },
        sized(conversation)
    ));
    for message in &request.messages {
        out.push_str(&message_line(message));
        out.push('\n');
    }
    let pictures = request.images.len();
    if pictures > 0 {
        out.push_str(&format!("  pictures shown: {pictures}\n"));
    }

    let all = system + tools + conversation;
    out.push_str(&format!(
        "\nIn all: {}, before an answer of at most {} tokens.\n",
        sized(all),
        grouped(request.max_tokens as usize)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolSpec;
    use serde_json::json;

    #[test]
    fn a_request_is_shown_with_its_sizes_and_nothing_is_asked_of_anyone() {
        let request = Request {
            system: "You keep the books.".into(),
            messages: vec![Message::user("What is my balance?")],
            tools: vec![
                ToolSpec {
                    name: "balance".into(),
                    description: "The account's balance. In its smallest unit.".into(),
                    input_schema: json!({"type": "object"}),
                },
                ToolSpec {
                    name: "pay".into(),
                    description: "Pays a payee of the charter".into(),
                    input_schema: json!({"type": "object", "properties": {"to": {"type": "string"}}}),
                },
            ],
            max_tokens: 16_384,
            images: Default::default(),
        };
        let answer = Shown::default().complete(&request).unwrap();
        assert_eq!(answer.stop, Stop::EndTurn);
        assert_eq!(answer.usage, Usage::default());
        let Block::Text { text } = &answer.content[0] else {
            panic!("its answer is text");
        };
        assert!(text.contains("System prompt: 19 characters, about 7 tokens"));
        assert!(text.contains("--------\nYou keep the books.\n--------"));
        assert!(text.contains("Tools: 2, "));
        assert!(text.contains("  balance      44 +     24  The account's balance."));
        assert!(text.contains("  pay    "));
        assert!(text.contains("Conversation: 1 message, 19 characters, about 7 tokens"));
        assert!(text.contains("  user: text (19)"));
        assert!(text.contains("before an answer of at most 16,384 tokens."));
        // Whole, each tool's description and schema.
        let full = render(&request, true);
        assert!(full.contains("pay (description 27, schema "));
        assert!(full.contains("\"to\": {"));
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
        assert_eq!(tokens(10), 4);
    }
}
