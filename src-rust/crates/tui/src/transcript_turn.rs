use crate::app::{App, ToolStatus, ToolUseBlock, TurnMetadata};
use claurst_core::types::{ContentBlock, Message, Role};

#[derive(Debug)]
pub struct TranscriptTurn<'a> {
    pub ordinal: usize,
    pub user_index: usize,
    pub end_message_index: usize,
    pub user_message: &'a Message,
    pub assistant_messages: Vec<(usize, &'a Message)>,
    pub tool_blocks: Vec<&'a ToolUseBlock>,
    pub live_text: Option<&'a str>,
    pub live_thinking: Option<&'a str>,
    pub metadata: Option<&'a TurnMetadata>,
    pub active: bool,
}

impl<'a> TranscriptTurn<'a> {
    pub fn last_assistant_index(&self) -> Option<usize> {
        self.assistant_messages.last().map(|(index, _)| *index)
    }

    pub fn primary_message_index(&self) -> usize {
        self.last_assistant_index().unwrap_or(self.user_index)
    }

    pub fn has_visible_assistant_content(&self) -> bool {
        !self.assistant_messages.is_empty()
            || !self.tool_blocks.is_empty()
            || self.live_text.is_some()
            || self.live_thinking.is_some()
    }

    pub fn reasoning_heading(&self) -> Option<String> {
        if let Some(text) = self.live_thinking.and_then(reasoning_heading) {
            return Some(text);
        }

        for (_, message) in self.assistant_messages.iter().rev() {
            for block in message.content_blocks().into_iter().rev() {
                if let ContentBlock::Thinking { thinking, .. } = block {
                    if let Some(text) = reasoning_heading(&thinking) {
                        return Some(text);
                    }
                }
            }
        }

        None
    }
}

pub fn reasoning_heading(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|line| !line.is_empty())?;

    let cleaned = first
        .trim_start_matches(['#', '*', '-', '>', ' '])
        .trim_start_matches("Thinking:")
        .trim();
    if cleaned.is_empty() {
        return None;
    }

    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }

    let mut chars = collapsed.chars();
    let preview: String = chars.by_ref().take(72).collect();
    if chars.next().is_some() {
        Some(format!("{preview}…"))
    } else {
        Some(preview)
    }
}

pub fn build_transcript_turns(app: &App) -> Vec<TranscriptTurn<'_>> {
    #[derive(Debug)]
    struct DraftTurn {
        ordinal: usize,
        user_index: usize,
        end_message_index: usize,
        assistant_indices: Vec<usize>,
    }

    let mut drafts = Vec::new();
    let mut current: Option<DraftTurn> = None;
    let mut ordinal = 0usize;

    for (index, message) in app.messages.iter().enumerate() {
        match message.role {
            Role::User => {
                if let Some(turn) = current.take() {
                    drafts.push(turn);
                }

                current = Some(DraftTurn {
                    ordinal,
                    user_index: index,
                    end_message_index: index,
                    assistant_indices: Vec::new(),
                });
                ordinal += 1;
            }
            Role::Assistant => {
                if let Some(turn) = current.as_mut() {
                    turn.assistant_indices.push(index);
                    turn.end_message_index = index;
                }
            }
        }
    }

    if let Some(turn) = current.take() {
        drafts.push(turn);
    }

    let mut turns: Vec<TranscriptTurn<'_>> = drafts
        .into_iter()
        .filter_map(|draft| {
            let user_message = app.messages.get(draft.user_index)?;
            Some(TranscriptTurn {
                ordinal: draft.ordinal,
                user_index: draft.user_index,
                end_message_index: draft.end_message_index,
                user_message,
                assistant_messages: draft
                    .assistant_indices
                    .into_iter()
                    .filter_map(|index| app.messages.get(index).map(|message| (index, message)))
                    .collect(),
                tool_blocks: Vec::new(),
                live_text: None,
                live_thinking: None,
                metadata: app.turn_metadata.get(draft.ordinal),
                active: false,
            })
        })
        .collect();

    for block in &app.tool_use_blocks {
        if let Some(target) = block
            .turn_index
            .and_then(|ordinal| turns.iter_mut().find(|turn| turn.ordinal == ordinal))
        {
            target.tool_blocks.push(block);
            continue;
        }

        if let Some(last) = turns.last_mut() {
            last.tool_blocks.push(block);
        }
    }

    if let Some(last) = turns.last_mut() {
        if !app.streaming_text.is_empty() {
            last.live_text = Some(app.streaming_text.as_str());
        }
        if !app.streaming_thinking.is_empty() {
            last.live_thinking = Some(app.streaming_thinking.as_str());
        }

        last.active = app.is_streaming
            || last
                .tool_blocks
                .iter()
                .any(|block| block.status == ToolStatus::Running);
    }

    turns
}
