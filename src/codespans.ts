/**
 * Splits plain text (what the user typed) into prose and code, the way chat
 * clients show it: ```fenced``` blocks and `inline` spans. Used for the
 * user's own bubbles and for the highlight behind the message input.
 * An unclosed backtick stays plain text.
 */
export type Span = { kind: "text" | "code" | "block"; text: string };

const FENCE = /```[^\n]*\n?([\s\S]*?)```/g;
const INLINE = /`([^`\n]+)`/g;

function inline(text: string, out: Span[]) {
  let at = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > at) out.push({ kind: "text", text: text.slice(at, m.index) });
    out.push({ kind: "code", text: m[0] });
    at = m.index + m[0].length;
  }
  if (at < text.length) out.push({ kind: "text", text: text.slice(at) });
}

/** Spans keep every character of the input (backticks included), so joining them gives the text back. */
export function codeSpans(text: string): Span[] {
  const out: Span[] = [];
  let at = 0;
  for (const m of text.matchAll(FENCE)) {
    if (m.index > at) inline(text.slice(at, m.index), out);
    out.push({ kind: "block", text: m[0] });
    at = m.index + m[0].length;
  }
  if (at < text.length) inline(text.slice(at), out);
  return out;
}

/** The code inside a span, without its backticks / fence line. */
export function codeBody(s: Span): string {
  if (s.kind === "code") return s.text.slice(1, -1);
  if (s.kind === "block") return s.text.replace(/^```[^\n]*\n?/, "").replace(/\n?```$/, "");
  return s.text;
}
