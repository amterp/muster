//! A message's Markdown, parsed once into a small tree any shell can draw (MIP-9, section 7).
//!
//! Agents write Markdown, and the human reads it in views the shell draws rather than in a
//! terminal, so somebody has to decide what `**done**` or a fenced block is. That is decided
//! here, where the core's suite can see it, and not in each platform's shell: a macOS shell
//! that leaned on `AttributedString(markdown:)` would make the rules Apple's, and a Linux shell
//! would have to guess them again. The tree names no platform type, and crosses the seam as
//! protobuf.
//!
//! It holds only what a message needs - paragraphs, lists, code blocks, and runs of text that
//! are emphasized, strong, code or a link - so everything else CommonMark can say is folded
//! into those: a heading is a strong paragraph, a quote is the blocks inside it, HTML is the
//! text it was written as.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Run>),
    List(List),
    Code(Code),
}

/// A bulleted list, or a numbered one counting from `start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub start: Option<u64>,
    pub items: Vec<Vec<Block>>,
}

/// A code block, with the language its fence named, empty when it named none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub language: String,
    pub text: String,
}

/// Text in one style. A hard line break is a `\n` inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub emphasis: bool,
    pub strong: bool,
    pub code: bool,
    pub link: Option<String>,
}

impl Run {
    fn same_style(&self, other: &Run) -> bool {
        (self.emphasis, self.strong, self.code, &self.link)
            == (other.emphasis, other.strong, other.code, &other.link)
    }
}

/// Parses `source` as CommonMark.
pub fn parse(source: &str) -> Vec<Block> {
    let mut builder = Builder { frames: vec![Frame::Blocks(Vec::new())], ..Builder::default() };
    for event in Parser::new(source) {
        builder.take(event);
    }
    builder.finish()
}

/// The first `lines` lines of `blocks`, and whether anything was left out.
///
/// A line is what the source would show on one: each line of a paragraph between hard breaks,
/// each line of a code block, and at least one per list item. A long paragraph without breaks
/// is one line here and several on screen, which the shell's own wrapping decides.
pub fn leading(blocks: &[Block], lines: usize) -> (Vec<Block>, bool) {
    let mut budget = lines;
    let kept = take_blocks(blocks, &mut budget);
    let cut = kept != blocks;
    (kept, cut)
}

fn take_blocks(blocks: &[Block], budget: &mut usize) -> Vec<Block> {
    let mut kept = Vec::new();
    for block in blocks {
        if *budget == 0 {
            break;
        }
        match block {
            Block::Paragraph(runs) => kept.push(Block::Paragraph(take_runs(runs, budget))),
            Block::Code(code) => {
                let all: Vec<&str> = code.text.split('\n').collect();
                let taken = all.len().min(*budget);
                *budget -= taken;
                kept.push(Block::Code(Code {
                    language: code.language.clone(),
                    text: all[..taken].join("\n"),
                }));
            }
            Block::List(list) => {
                let mut items = Vec::new();
                for item in &list.items {
                    if *budget == 0 {
                        break;
                    }
                    let item_kept = take_blocks(item, budget);
                    if item_kept.is_empty() {
                        // An empty item still takes its line.
                        *budget -= 1;
                    }
                    items.push(item_kept);
                }
                kept.push(Block::List(List { start: list.start, items }));
            }
        }
    }
    kept
}

fn take_runs(runs: &[Run], budget: &mut usize) -> Vec<Run> {
    let mut kept: Vec<Run> = Vec::new();
    *budget -= 1;
    for run in runs {
        let mut text = String::new();
        let mut stopped = false;
        for (index, piece) in run.text.split('\n').enumerate() {
            if index > 0 {
                if *budget == 0 {
                    stopped = true;
                    break;
                }
                *budget -= 1;
                text.push('\n');
            }
            text.push_str(piece);
        }
        if !text.is_empty() {
            kept.push(Run { text, ..run.clone() });
        }
        if stopped {
            break;
        }
    }
    kept
}

/// Where blocks are being collected: the message itself, or a list item inside it.
#[derive(Debug)]
enum Frame {
    Blocks(Vec<Block>),
    List { start: Option<u64>, items: Vec<Vec<Block>> },
}

#[derive(Debug, Default)]
struct Builder {
    frames: Vec<Frame>,
    /// The paragraph being written, opened by the first inline text when the source has no
    /// paragraph around it, as a tight list item's text does.
    runs: Option<Vec<Run>>,
    code: Option<Code>,
    emphasis: usize,
    strong: usize,
    headings: usize,
    links: Vec<String>,
}

impl Builder {
    fn take(&mut self, event: Event<'_>) {
        if let Some(code) = &mut self.code {
            match event {
                Event::Text(text) => code.text.push_str(&text),
                Event::End(TagEnd::CodeBlock) => {
                    let mut code = self.code.take().unwrap_or_else(|| unreachable!());
                    if code.text.ends_with('\n') {
                        code.text.pop();
                    }
                    self.push_block(Block::Code(code));
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, false),
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.text(&text, true);
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                self.text(text.trim_end_matches('\n'), false);
            }
            Event::FootnoteReference(name) => self.text(&format!("[^{name}]"), false),
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }, false),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => self.close_paragraph(),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.close_paragraph(),
            Tag::Heading { .. } => {
                self.close_paragraph();
                self.headings += 1;
            }
            Tag::CodeBlock(kind) => {
                self.close_paragraph();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or_default().to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(Code { language, text: String::new() });
            }
            Tag::List(start) => {
                self.close_paragraph();
                self.frames.push(Frame::List { start, items: Vec::new() });
            }
            Tag::Item => {
                self.close_paragraph();
                self.frames.push(Frame::Blocks(Vec::new()));
            }
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push(dest_url.to_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.close_paragraph(),
            TagEnd::Heading(_) => {
                self.close_paragraph();
                self.headings = self.headings.saturating_sub(1);
            }
            TagEnd::Item => {
                self.close_paragraph();
                if let Some(Frame::Blocks(blocks)) = self.frames.pop()
                    && let Some(Frame::List { items, .. }) = self.frames.last_mut()
                {
                    items.push(blocks);
                }
            }
            TagEnd::List(_) => {
                self.close_paragraph();
                if let Some(Frame::List { start, items }) = self.frames.pop() {
                    self.push_block(Block::List(List { start, items }));
                }
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let run = Run {
            text: text.to_string(),
            emphasis: self.emphasis > 0,
            strong: self.strong > 0 || self.headings > 0,
            code,
            link: self.links.last().cloned(),
        };
        let runs = self.runs.get_or_insert_with(Vec::new);
        match runs.last_mut() {
            Some(last) if last.same_style(&run) => last.text.push_str(&run.text),
            _ => runs.push(run),
        }
    }

    fn close_paragraph(&mut self) {
        if let Some(runs) = self.runs.take().filter(|runs| !runs.is_empty()) {
            self.push_block(Block::Paragraph(runs));
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.frames.last_mut() {
            Some(Frame::Blocks(blocks)) => blocks.push(block),
            // Blocks arrive only inside a list's items, never in the list itself, and the
            // message's own frame is never popped.
            Some(Frame::List { .. }) | None => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_paragraph();
        match self.frames.into_iter().next() {
            Some(Frame::Blocks(blocks)) => blocks,
            _ => Vec::new(),
        }
    }
}
