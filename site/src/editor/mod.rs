mod utils;

use std::{cell::Cell, rc::Rc, time::Duration};

use base64::engine::{
    general_purpose::{STANDARD, URL_SAFE},
    Engine,
};

use leptos::{ev::keydown, *};
use leptos_router::{use_navigate, BrowserIntegration, History, LocationChange, NavigateOptions};
use uiua::{
    format::{format_str, FormatConfig},
    is_ident_char, Primitive, SysOp,
};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    Event, HtmlDivElement, HtmlInputElement, HtmlSelectElement, KeyboardEvent, MouseEvent,
};

use crate::{backend::OutputItem, element, prim_class, Prim};

use utils::*;
pub use utils::{get_ast_time, Challenge};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorSize {
    #[default]
    Small,
    Medium,
    Pad,
}

#[derive(Default)]
pub enum EditorMode {
    #[default]
    Progressive,
    Multiple,
}

thread_local! {
    static ID: Cell<u64> = Cell::new(0);
}

/// An editor for Uiua code
#[component]
pub fn Editor<'a>(
    #[prop(optional)] example: &'a str,
    #[prop(optional)] examples: &'a [&'a str],
    #[prop(optional)] size: EditorSize,
    #[prop(optional)] help: &'a [&'a str],
    #[prop(optional)] mode: EditorMode,
    #[prop(optional)] progress_lines: bool,
    #[prop(optional)] no_run: bool,
    #[prop(optional)] challenge: Option<ChallengeDef>,
) -> impl IntoView {
    let no_run = no_run
        || ["&sl", "&httpsw", "send", "recv"]
            .iter()
            .any(|name| example.contains(name));
    let id = ID.with(|id| {
        let i = id.get();
        id.set(i + 1);
        i
    });
    let help: Vec<String> = help.iter().map(|s| s.to_string()).collect();
    // Initialize all the examples
    let examples = Some(example)
        .filter(|ex| !ex.is_empty() || examples.is_empty())
        .into_iter()
        .chain(examples.iter().copied());
    let examples: Vec<String> = match mode {
        EditorMode::Progressive => {
            let mut examples: Vec<_> = if progress_lines {
                examples
                    .scan(String::new(), |acc, s| {
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(s);
                        Some(acc.clone())
                    })
                    .collect()
            } else {
                examples
                    .rev()
                    .scan(String::new(), |acc, s| {
                        acc.insert_str(0, s);
                        Some(acc.clone())
                    })
                    .collect()
            };
            examples.rotate_right(1);
            examples
        }
        EditorMode::Multiple => examples.map(Into::into).collect(),
    };
    let code_max_lines = if let EditorSize::Pad = size {
        10
    } else if let Some(chal) = &challenge {
        chal.answer.lines().count()
    } else {
        examples.iter().map(|e| e.lines().count()).max().unwrap()
    };
    let code_height_em = code_max_lines as f32 * 1.2;

    let code_id = move || format!("code{id}");
    let glyph_doc_id = move || format!("glyphdoc{id}");

    let code_element = move || -> HtmlDivElement { element(&code_id()) };
    let glyph_doc_element = move || -> HtmlDivElement { element(&glyph_doc_id()) };

    // Track line count
    let (line_count, set_line_count) = create_signal(1);

    let (initial_code, set_initial_code) = create_signal(Some(
        examples.get(0).cloned().unwrap_or_else(|| example.into()),
    ));

    let (example, set_example) = create_signal(0);
    let (output, set_output) = create_signal(View::default());

    let code_text = move || code_text(&code_id());
    let get_code_cursor = move || get_code_cursor_impl(&code_id());
    let (copied_link, set_copied_link) = create_signal(false);
    let (settings_open, set_settings_open) = create_signal(false);

    // Initialize the state
    let state = Rc::new(State {
        code_id: code_id(),
        set_line_count,
        set_copied_link,
        past: Default::default(),
        future: Default::default(),
        challenge,
        curr: {
            let code = initial_code.get_untracked().unwrap();
            let len = code.chars().count() as u32;
            Record {
                code,
                before: (len, len),
                after: (len, len),
            }
        }
        .into(),
    });
    let (state, _) = create_signal(state);
    let state = move || state.get();

    // Run the code
    let run = move |format: bool, set_cursor: bool| {
        // Get code
        let mut code_text = code_text();
        let mut cursor = if set_cursor {
            Cursor::Keep
        } else {
            Cursor::Ignore
        };
        if let Some(code) = initial_code.get() {
            code_text = code;
            set_initial_code.set(None);
            cursor = Cursor::Ignore;
        }

        // Format code
        let input = if format {
            if let Ok(formatted) = format_str(
                &code_text,
                &FormatConfig {
                    trailing_newline: size == EditorSize::Pad,
                    ..Default::default()
                },
            ) {
                let cursor = if let Some((start, end)) = get_code_cursor() {
                    let (new_start_start, new_start_end) = formatted.map_char_pos(start as usize);
                    let (new_end_start, new_end_end) = formatted.map_char_pos(end as usize);
                    let (new_start, new_end) = if get_right_to_left() {
                        (new_start_start, new_end_start)
                    } else {
                        (new_start_end, new_end_end)
                    };
                    Cursor::Set(new_start as u32, new_end as u32)
                } else {
                    cursor
                };
                state().set_code(&formatted.output, cursor);
                formatted.output
            } else {
                state().set_code(&code_text, cursor);
                code_text
            }
        } else {
            state().set_code(&code_text, cursor);
            code_text
        };

        // Update URL
        {
            let encoded = URL_SAFE.encode(&input);
            if let EditorSize::Pad = size {
                BrowserIntegration {}.navigate(&LocationChange {
                    value: format!("/pad?src={encoded}"),
                    scroll: false,
                    replace: true,
                    ..Default::default()
                });
            }
        }

        // Run code
        set_output.set(view!(<div class="running-text">"Running"</div>).into_view());
        set_timeout(
            move || {
                let output = state().run_code(&input);
                let mut allow_autoplay = !matches!(size, EditorSize::Small);
                let render_output_item = |item| match item {
                    OutputItem::String(s) => {
                        if s.is_empty() {
                            view!(<div class="output-item"><br/></div>).into_view()
                        } else {
                            view!(<div class="output-item">{s}</div>).into_view()
                        }
                    }
                    OutputItem::Faint(s) => {
                        view!(<div class="output-item output-fainter">{s}</div>).into_view()
                    }
                    OutputItem::Image(bytes) => {
                        let encoded = STANDARD.encode(bytes);
                        view!(<div><img class="output-image" src={format!("data:image/png;base64,{encoded}")} /></div>).into_view()
                    }
                    OutputItem::Gif(bytes) => {
                        let encoded = STANDARD.encode(bytes);
                        view!(<div><img class="output-image" src={format!("data:image/gif;base64,{encoded}")} /></div>).into_view()
                    }
                    OutputItem::Audio(bytes) => {
                        let encoded = STANDARD.encode(bytes);
                        let src = format!("data:audio/wav;base64,{}", encoded);
                        if allow_autoplay {
                            allow_autoplay = false;
                            view!(<div><audio class="output-audio" controls autoplay src=src/></div>).into_view()
                        } else {
                            view!(<div><audio class="output-audio" controls src=src/></div>)
                                .into_view()
                        }
                    }
                    OutputItem::Report(report) => report_view(&report).into_view(),
                    OutputItem::Separator => {
                        view!(<div class="output-item"><hr/></div>).into_view()
                    }
                };
                let items: Vec<_> = output.into_iter().map(render_output_item).collect();
                set_output.set(items.into_view());
            },
            Duration::ZERO,
        );
    };

    // Replace the selected text in the editor with the given string
    let replace_code = move |inserted: &str| {
        if let Some((start, end)) = get_code_cursor() {
            let (start, end) = (start.min(end), start.max(end) as usize);
            let code = code_text();
            let new: String = code
                .chars()
                .take(start as usize)
                .chain(inserted.chars())
                .chain(code.chars().skip(end))
                .collect();
            let offset = inserted.chars().count() as u32;
            state().set_code(&new, Cursor::Set(start + offset, start + offset));
        };
    };

    // Remove a code range
    let remove_code = move |start: u32, end: u32| {
        // logging::log!("start: {start}, end: {end}");
        if start == end {
            return;
        }
        let (start, end) = (start.min(end), start.max(end) as usize);
        let code = code_text();
        let new: String = code
            .chars()
            .take(start as usize)
            .chain(code.chars().skip(end))
            .collect();
        state().set_code(&new, Cursor::Set(start, start));
    };

    // Surround the selected text with delimiters
    let surround_code = move |open: char, close: char| {
        let (start, end) = get_code_cursor().unwrap();
        let (start, end) = (start.min(end), start.max(end));
        let code = code_text();
        let mut chars = code.chars();
        let mut new_code = String::new();
        new_code.extend(chars.by_ref().take(start as usize));
        new_code.push(open);
        new_code.extend(chars.by_ref().take((end - start) as usize));
        new_code.push(close);
        new_code.extend(chars);
        state().set_code(&new_code, Cursor::Set(start + 1, end + 1));
    };

    // Update the code when the textarea is changed
    let code_input = move |event: Event| {
        let event = event.dyn_into::<web_sys::InputEvent>().unwrap();
        let parent = code_element();
        let child: HtmlDivElement = event.target().unwrap().dyn_into().unwrap();
        if !parent.contains(Some(&child)) {
            return;
        }
        if let Some((start, _)) = get_code_cursor() {
            state().set_code(&code_text(), Cursor::Set(start, start));
        }
    };

    let on_mac = window()
        .navigator()
        .user_agent()
        .unwrap()
        .to_lowercase()
        .contains("mac");
    let os_ctrl = move |event: &KeyboardEvent| {
        if on_mac {
            event.meta_key()
        } else {
            event.ctrl_key()
        }
    };

    // Handle key events
    window_event_listener(keydown, move |event| {
        let event = event.dyn_ref::<web_sys::KeyboardEvent>().unwrap();
        let focused = event
            .target()
            .and_then(|t| t.dyn_into::<HtmlDivElement>().ok())
            .is_some_and(|t| t.id() == code_id());
        if !focused {
            return;
        }
        let mut handled = true;
        /// For determining if ctrl+backspace/delete should remove a sequence of characters
        fn char_class(c: char) -> u8 {
            if is_ident_char(c) {
                1
            } else if c.is_ascii_digit() {
                2
            } else if c == '\n' {
                3
            } else if c == '\'' {
                4
            } else if c == '"' {
                5
            } else if "()[]{}".contains(c) {
                7
            } else {
                0
            }
        }
        let key = event.key();
        let key = key.as_str();
        match key {
            "Enter" => {
                if os_ctrl(event) || event.shift_key() {
                    run(true, true);
                } else {
                    replace_code("\n");
                }
            }
            "Backspace" => {
                let (start, end) = get_code_cursor().unwrap();
                // logging::log!("backspace start: {start}, end: {end}");
                if start == end {
                    if start > 0 {
                        let mut removal_count = 1;
                        if os_ctrl(event) {
                            removal_count = 0;
                            let code = code_text();
                            let chars: Vec<_> = code.chars().take(start as usize).collect();
                            let last_char = *chars.last().unwrap();
                            let class = char_class(last_char);
                            let mut encountered_space = false;
                            for &c in chars.iter().rev() {
                                if c.is_whitespace() && c != '\n'
                                    || char_class(c) == class && !encountered_space
                                {
                                    removal_count += 1;
                                } else {
                                    break;
                                }
                                encountered_space |= c.is_whitespace();
                            }
                        }
                        remove_code(start - removal_count, start);
                    }
                } else {
                    remove_code(start, end);
                }
            }
            "Delete" if event.shift_key() => {
                // Delete lines between cursor start and end
                let code = code_text();
                let (start, end) = get_code_cursor().unwrap();
                let (start, end) = (start.min(end), start.max(end));
                let (start_line, _) = line_col(&code, start as usize);
                let (end_line, _) = line_col(&code, end as usize);
                let new_code: String = code
                    .lines()
                    .enumerate()
                    .filter_map(|(i, line)| {
                        if i < start_line - 1 || i >= end_line {
                            if i == 0 || start_line == 1 && i == end_line {
                                Some(line.into())
                            } else {
                                Some(format!("\n{}", line))
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                state().set_code(&new_code, Cursor::Set(start, start));
            }
            "Delete" => {
                let (start, end) = get_code_cursor().unwrap();
                if start == end {
                    let mut removal_count = 1;
                    if os_ctrl(event) {
                        removal_count = 0;
                        let code = code_text();
                        let chars: Vec<_> = code.chars().skip(end as usize).collect();
                        let first_char = *chars.first().unwrap();
                        let class = char_class(first_char);
                        let mut encountered_space = false;
                        for &c in chars.iter() {
                            if c.is_whitespace() && c != '\n'
                                || char_class(c) == class && !encountered_space
                            {
                                removal_count += 1;
                            } else {
                                break;
                            }
                            encountered_space |= c.is_whitespace();
                        }
                    }
                    remove_code(start, start + removal_count);
                } else {
                    remove_code(start, end);
                }
            }
            "Tab" => {
                replace_code("\t");
            }
            // Select all
            "a" if os_ctrl(event) => {
                let code = code_text();
                state().set_code(&code, Cursor::Set(0, code.chars().count() as u32));
            }
            // Copy
            "c" if os_ctrl(event) => {
                let (start, end) = get_code_cursor().unwrap();
                let (start, end) = (start.min(end), start.max(end));
                let code = code_text();
                let text: String = code
                    .chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect();
                _ = window().navigator().clipboard().unwrap().write_text(&text);
            }
            // Cut
            "x" if os_ctrl(event) => {
                let (start, end) = get_code_cursor().unwrap();
                let (start, end) = (start.min(end), start.max(end));
                let code = code_text();
                let text: String = code
                    .chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect();
                _ = window().navigator().clipboard().unwrap().write_text(&text);
                remove_code(start, end);
            }
            // Undo
            "z" if os_ctrl(event) => state().undo(),
            // Redo
            "y" if os_ctrl(event) => state().redo(),
            // Toggle line comment
            "/" if os_ctrl(event) => {
                let code = code_text();
                let (start, end) = get_code_cursor().unwrap();
                let (start, end) = (start.min(end), start.max(end));
                let (start_line, _) = line_col(&code, start as usize);
                let (end_line, _) = line_col(&code, end as usize);
                let mut lines: Vec<String> = code.lines().map(Into::into).collect();
                let range = &mut lines[start_line - 1..end_line];
                if range.iter().all(|line| line.trim().starts_with('#')) {
                    // Toggle comments off
                    for line in range {
                        if line.starts_with("# ") {
                            line.replace_range(0..2, "");
                        } else {
                            *line = line.trim_start_matches('#').into();
                        }
                    }
                } else {
                    // Toggle comments on
                    for line in range {
                        line.insert_str(0, "# ");
                    }
                }
                let new_code = lines.join("\n");
                state().set_code(&new_code, Cursor::Set(start, end));
            }
            // Handle double quote delimiters
            "\"" => {
                let (start, end) = get_code_cursor().unwrap();
                let code = code_text();
                if start != end
                    || code
                        .chars()
                        .nth(start as usize)
                        .map_or(true, |c| c.is_whitespace())
                {
                    surround_code('"', '"');
                } else if start == end && code_text().chars().nth(start as usize) == Some('"') {
                    state().set_cursor((start + 1, start + 1));
                } else {
                    replace_code(key);
                }
            }
            // Handle open delimiters
            "(" | "[" | "{" => {
                // Surround the selected text with delimiters
                let (open, close) = match key {
                    "\"" => ('"', '"'),
                    "(" => ('(', ')'),
                    "[" => ('[', ']'),
                    "{" => ('{', '}'),
                    _ => unreachable!(),
                };
                let (start, end) = get_code_cursor().unwrap();
                if start != end
                    || code_text()
                        .chars()
                        .nth(start as usize)
                        .map_or(true, |c| c.is_whitespace())
                {
                    surround_code(open, close);
                } else {
                    replace_code(key);
                }
            }
            // Handle close delimiters
            ")" | "]" | "}" => {
                let (start, end) = get_code_cursor().unwrap();
                let close = key.chars().next().unwrap();
                if start == end && code_text().chars().nth(start as usize) == Some(close) {
                    state().set_cursor((start + 1, start + 1));
                } else {
                    handled = false;
                }
            }
            // Line swapping with alt+up/down
            key @ ("ArrowUp" | "ArrowDown") if event.alt_key() => {
                let (_, end) = get_code_cursor().unwrap();
                let code = code_text();
                let (line, col) = line_col(&code, end as usize);
                let line_index = line - 1;
                let up = key == "ArrowUp";
                let mut lines: Vec<String> = code.lines().map(Into::into).collect();
                if up && line_index > 0 || !up && line_index < lines.len() - 1 {
                    let swap_index = if up { line_index - 1 } else { line_index + 1 };
                    lines.swap(line_index, swap_index);
                    let swapped: String = lines.join("\n");
                    let mut new_end = 0;
                    for (i, line) in lines.iter().enumerate() {
                        if i == swap_index {
                            let line_len = line.chars().count();
                            if col < line_len {
                                new_end += col as u32;
                                new_end -= 1;
                            } else {
                                new_end += line_len as u32;
                            }
                            break;
                        }
                        new_end += line.chars().count() as u32 + 1;
                    }
                    state().set_code(&swapped, Cursor::Set(new_end, new_end));
                }
            }
            // Intercept forward/back keyboard navigation
            "ArrowLeft" | "ArrowRight" if event.alt_key() => {}
            _ => handled = false,
        }
        if handled {
            event.prevent_default();
            event.stop_propagation();
        }
    });

    // Handle paste evens
    let code_paste = move |event: Event| {
        let event = event.dyn_into::<web_sys::ClipboardEvent>().unwrap();
        event.prevent_default();
        event.stop_propagation();
        let text = event.clipboard_data().unwrap().get_data("text").unwrap();
        replace_code(&text);
    };

    // Go to the next example
    let next_example = {
        let examples = examples.clone();
        move |_| {
            set_example.update(|e| {
                *e = (*e + 1) % examples.len();
                state().set_code(&examples[*e], Cursor::Ignore);
                state().clear_history();
                run(false, false);
            })
        }
    };
    // Go to the previous example
    let prev_example = {
        let examples = examples.clone();
        move |_| {
            set_example.update(|e| {
                *e = (*e + examples.len() - 1) % examples.len();
                state().set_code(&examples[*e], Cursor::Ignore);
                state().clear_history();
                run(false, false);
            })
        }
    };

    // Glyph hover doc
    let (glyph_doc, set_glyph_doc) = create_signal(View::default());
    let onmouseleave = move |_| {
        _ = glyph_doc_element().style().set_property("display", "none");
    };

    // Glyph buttons
    // These are the buttons that appear above the editor and allow the user to insert glyphs
    let make_glyph_button = |prim: Primitive| {
        let text = prim
            .glyph()
            .map(Into::into)
            .or_else(|| prim.ascii().map(|s| s.to_string()))?;
        let mut title = prim.name().to_string();
        if let Some(ascii) = prim.ascii() {
            title = format!("({}) {}", ascii, title);
        }
        // Navigate to the docs page on ctrl/shift+click
        let onclick = move |event: MouseEvent| {
            if !on_mac && event.ctrl_key() || on_mac && event.meta_key() {
                // Open the docs page
                window()
                    .open_with_url_and_target(&format!("/docs/{}", prim.name()), "_blank")
                    .unwrap();
            } else if event.shift_key() {
                // Redirect to the docs page
                use_navigate()(
                    &format!("/docs/{}", prim.name()),
                    NavigateOptions::default(),
                );
            } else {
                replace_code(&prim.to_string());
            }
        };
        // Show the glyph doc on mouseover
        let onmouseover = move |_| {
            if let Some(doc) = prim.doc() {
                set_glyph_doc.set(
                    view! {
                        <Prim prim=prim/>
                        <br/>
                        { doc.short_text().into_owned() }
                    }
                    .into_view(),
                );
                _ = glyph_doc_element().style().remove_property("display");
            }
        };
        Some(
            view! {
                <button
                    class="glyph-button glyph-title"
                    data-title=title
                    on:click=onclick
                    on:mouseover=onmouseover
                    on:mouseleave=onmouseleave>
                    <div class={prim_class(prim)}>{ text }</div>
                </button>
            }
            .into_view(),
        )
    };
    let mut glyph_buttons: Vec<_> = Primitive::non_deprecated()
        .filter_map(make_glyph_button)
        .collect();

    // Additional code buttons
    for (glyph, title, class, surround, doc) in [
        ("_", "strand", "strand-span", None, "arrays#creating-arrays"),
        (
            "[]",
            "array",
            "",
            Some(('[', ']')),
            "arrays#creating-arrays",
        ),
        (
            "{}",
            "box array",
            "",
            Some(('{', '}')),
            "arrays#nested-arrays",
        ),
        (
            "()",
            "function",
            "",
            Some(('(', ')')),
            "functions#inline-functions",
        ),
        ("¯", "negative (`)", "number-literal-span", None, ""),
        (
            "@",
            "character",
            "string-literal-span",
            None,
            "types#characters",
        ),
        (
            "$",
            "format/multiline string",
            "string-literal-span",
            None,
            "functions#format-strings",
        ),
        (
            "\"",
            "string",
            "string-literal-span",
            Some(('"', '"')),
            "types#characters",
        ),
        ("^", "placeholder", "", None, "custommodifiers"),
        ("←", "binding (=)", "", None, "bindings"),
        ("|", "signature", "", None, "functions#stack-signatures"),
        ("#", "comment", "comment-span", None, "basic#comments"),
    ] {
        let class = format!("glyph-button {class}");
        // Navigate to the docs page on ctrl/shift+click
        let onclick = move |event: MouseEvent| {
            if !doc.is_empty() && (!on_mac && event.ctrl_key() || on_mac && event.meta_key()) {
                // Open the docs page
                window()
                    .open_with_url_and_target(&format!("/docs/{doc}"), "_blank")
                    .unwrap();
            } else if !doc.is_empty() && event.shift_key() {
                // Redirect to the docs page
                use_navigate()(&format!("/docs/{doc}"), NavigateOptions::default());
            } else if let Some((open, close)) = surround {
                surround_code(open, close);
            } else {
                replace_code(glyph)
            }
        };
        // Show the doc on mouseover
        let onmouseover = move |_| {
            if !doc.is_empty() {
                set_glyph_doc.set(view!(<code>{ glyph }</code>" "{ title }).into_view());
                _ = glyph_doc_element().style().remove_property("display");
            }
        };
        glyph_buttons.push(
            view! {
                <button
                    class=class
                    data-title=title
                    on:click=onclick
                    on:mouseover=onmouseover
                    on:mouseleave=onmouseleave>
                    {glyph}
                </button>
            }
            .into_view(),
        );
    }

    // Select a class for the editor and code area
    let editor_class = match size {
        EditorSize::Small => "small-editor",
        EditorSize::Medium | EditorSize::Pad => "medium-editor",
    };

    // Hide the example arrows if there is only one example
    let example_arrow_style = if examples.len() <= 1 {
        "display:none"
    } else {
        ""
    };

    // Show or hide the glyph buttons
    let (show_glyphs, set_show_glyphs) = create_signal(match size {
        EditorSize::Small => false,
        EditorSize::Medium | EditorSize::Pad => true,
    });

    // Glyphs toggle button
    let show_glyphs_text = move || if show_glyphs.get() { "↥" } else { "↧" };
    let show_glyphs_title = move || {
        if show_glyphs.get() {
            "Hide glyphs"
        } else {
            "Show glyphs"
        }
    };
    let toggle_show_glyphs = move |_| set_show_glyphs.update(|s| *s = !*s);

    // Hide the glyph buttons if the editor is small
    let glyph_buttons_style = move || {
        if show_glyphs.get() {
            ""
        } else {
            "display:none"
        }
    };

    // Show the example number if there are multiple examples
    let examples_len = examples.len();
    let example_text =
        move || (examples_len > 1).then(|| format!("{}/{}", example.get() + 1, examples_len));

    // Select a class for the next example button
    let next_button_class = move || {
        if example.get() == examples_len - 1 {
            "code-button"
        } else {
            "code-button important-button"
        }
    };

    // This ensures the output of the first example is shown
    set_timeout(
        move || {
            if no_run {
                let code = initial_code.get().unwrap();
                set_initial_code.set(None);
                state().set_code(&code, Cursor::Ignore);
            } else {
                run(false, false)
            }
        },
        Duration::from_millis(0),
    );

    // Line numbers
    let line_numbers = move || {
        (0..line_count.get().max(1))
            .map(|i| {
                view!( <div>
                    <span class="code-span line-number">{i + 1}</span>
                </div>)
            })
            .collect::<Vec<_>>()
    };

    // Copy a link to the code
    let copy_link = move |_| {
        let encoded = URL_SAFE.encode(code_text());
        let url = format!("https://uiua.org/pad?src={encoded}");
        _ = window().navigator().clipboard().unwrap().write_text(&url);
        if let EditorSize::Pad = size {
            window()
                .history()
                .unwrap()
                .push_state_with_url(&JsValue::NULL, "", Some(&format!("/pad?src={encoded}")))
                .unwrap();
        }
        set_copied_link.set(true);
    };
    let copy_link_title = move || {
        if copied_link.get() {
            "Copied!"
        } else {
            "Copy a link to this code"
        }
    };

    // Toggle settings
    let toggle_settings_open = move |_| {
        set_settings_open.update(|s| *s = !*s);
    };
    let toggle_settings_title = move || {
        if settings_open.get() {
            "Hide settings"
        } else {
            "Show settings"
        }
    };

    // Settings
    let settings_style = move || {
        if settings_open.get() {
            ""
        } else {
            "display:none"
        }
    };
    let on_execution_limit_change = move |event: Event| {
        let event = event.dyn_into::<web_sys::InputEvent>().unwrap();
        let input: HtmlInputElement = event.target().unwrap().dyn_into().unwrap();
        let limit = input.value().parse().unwrap_or(2.0);
        set_execution_limit(limit);
    };
    let on_ast_time_change = move |event: Event| {
        let event = event.dyn_into::<web_sys::InputEvent>().unwrap();
        let input: HtmlInputElement = event.target().unwrap().dyn_into().unwrap();
        let time = input.value().parse().unwrap_or(10.0);
        set_ast_time(time);
    };
    let toggle_right_to_left = move |_| {
        set_right_to_left(!get_right_to_left());
    };
    let on_select_font = move |event: Event| {
        let input: HtmlSelectElement = event.target().unwrap().dyn_into().unwrap();
        let name = input.value();
        set_font_name(&name);
    };
    let on_select_font_size = move |event: Event| {
        let input: HtmlSelectElement = event.target().unwrap().dyn_into().unwrap();
        let size = input.value();
        set_font_size(&size);
    };
    let on_select_top_at_top = move |event: Event| {
        let input: HtmlSelectElement = event.target().unwrap().dyn_into().unwrap();
        let orientation = input.value() == "true";
        set_top_at_top(orientation);
        run(false, false);
    };
    set_font_name(&get_font_name());
    set_font_size(&get_font_size());

    // Render
    view! {
        <div id="editor-wrapper">
            <div id="editor">
                <div style=glyph_buttons_style>
                    <div class="glyph-buttons">{glyph_buttons}</div>
                </div>
                <div id="settings" style=settings_style>
                    <div title="The maximum number of seconds a program can run for">
                        "Exec limit:"
                        <input
                            type="number"
                            min="0.01"
                            max="1000000"
                            width="3em"
                            value=get_execution_limit
                            on:input=on_execution_limit_change/>
                        "s"
                    </div>
                    <div title="The maximum number of seconds of audio &ast will generate">
                        <Prim prim=Primitive::Sys(SysOp::AudioStream) />" time:"
                        <input
                            type="number"
                            min="1"
                            max="600"
                            width="3em"
                            value=get_ast_time
                            on:input=on_ast_time_change/>
                        "s"
                    </div>
                    <div title="Place the cursor on the left of the current token when formatting">
                        "Format left:"
                        <input
                            type="checkbox"
                            checked=get_right_to_left
                            on:change=toggle_right_to_left/>
                    </div>
                    <div>
                        "Stack:"
                        <select
                            on:change=on_select_top_at_top>
                            <option value="false" selected=get_top_at_top()>"Top at bottom"</option>
                            <option value="true" selected=get_top_at_top()>"Top at top"</option>
                        </select>
                    </div>
                    <div>
                        "Font size:"
                        <select
                            on:change=on_select_font_size>
                            <option value="0.6em" selected={get_font_size() == "0.6em"}>"Scalar"</option>
                            <option value="0.8em" selected={get_font_size() == "0.8em"}>"Small"</option>
                            <option value="1em" selected={get_font_size() == "1em"}>"Normal"</option>
                            <option value="1.2em" selected={get_font_size() == "1.2em"}>"Big"</option>
                            <option value="1.4em" selected={get_font_size() == "1.4em"}>"Rank 3"</option>
                        </select>
                    </div>
                    <div>
                        "Font:"
                        <select
                            on:change=on_select_font>
                            <option value="DejaVuSansMono" selected={get_font_name() == "DejaVuSansMono"}>"DejaVu"</option>
                            <option value="Uiua386" selected={get_font_name() == "Uiua386"}>"Uiua386"</option>
                        </select>
                    </div>
                    <button
                        class="info-button"
                        data-title="\
shift Enter   - Run + Format
 ctrl /       - Toggle line comment
  alt Up/Down - Swap lines
shift Delete  - Delete lines
 ctrl Z       - Undo
 ctrl Y       - Redo"
                        disabled>
                        "🛈"
                    </button>
                </div>
                <div class=editor_class>
                    <div id="code-area">
                        <div id={glyph_doc_id} class="glyph-doc" style="display: none">
                            { move || glyph_doc.get() }
                            <div class="glyph-doc-ctrl-click">"Shift+click for more info (Ctrl+click for new tab)"</div>
                        </div>
                        <div id="code-right-side">
                            <button
                                class="editor-right-button"
                                data-title=copy_link_title
                                on:click=copy_link>
                                "🔗"
                            </button>
                            <button
                                id="glyphs-toggle-button"
                                class="editor-right-button"
                                data-title=show_glyphs_title
                                on:click=toggle_show_glyphs>{show_glyphs_text}
                            </button>
                            <button
                                class="editor-right-button"
                                data-title=toggle_settings_title
                                on:click=toggle_settings_open>
                                "⚙️"
                            </button>
                            <div id="example-tracker">{example_text}</div>
                        </div>
                        <div class="code sized-code">
                            <div class="line-numbers">
                                { line_numbers }
                            </div>
                            // The text entry area
                            <div
                                id={code_id}
                                contenteditable="true"
                                spellcheck="false"
                                class="code-entry"
                                style={format!("height: {code_height_em}em;")}
                                on:input=code_input
                                on:paste=code_paste>
                                "Loading..."
                            </div>
                        </div>
                    </div>
                    <div class="output-frame">
                        <div class="output sized-code">
                            { move || output.get() }
                        </div>
                        <div id="code-buttons">
                            <button class="code-button" on:click=move |_| run(true, false)>{ "Run" }</button>
                            <button
                                id="prev-example"
                                class="code-button"
                                style=example_arrow_style
                                on:click=prev_example>{ "<" } </button>
                            <button
                                id="next-example"
                                class=next_button_class
                                style=example_arrow_style
                                on:click=next_example>{ ">" } </button>
                        </div>
                    </div>
                </div>
            </div>
            <div id="editor-help">
                { help.iter().map(|s| view!(<p>{s}</p>)).collect::<Vec<_>>() }
            </div>
            <div id="editor-help">
            {
                if let EditorSize::Pad = size {
                    Some("Note: Uiua is not yet stable")
                } else {
                    None
                }
            }
            </div>
        </div>
    }
}