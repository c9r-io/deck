// Terminal path/URL tokens and public buffer geometry. No DOM, filesystem
// or view-core imports; hover resolution stays synchronous and bounded.
/* Terminal links are tokenized before they are classified. A URL consumes
   its complete interval first, so `/api` inside it can never become a path.
   Path tokens are only CANDIDATES: they are offered on their text alone so a
   hovered link cannot flicker, and the link actions resolve and validate them
   against the pane cwd (`links.rs`). ASCII prose wrappers end a candidate;
   balanced brackets within a filename are retained. Failed scans advance
   past the inspected span, so long non-path output cannot cause quadratic
   suffix rescans. Bracket matching and URL punctuation trimming are linear. */
const URL_SCHEMES = ['https://', 'http://'];
const PATH_START_DELIMS = '=:()[]{}<,;|';
const TOKEN_END_DELIMS = '"\'`<>|\\';
const PATH_HARD_END_DELIMS = '=,;';
// A final colon is label punctuation (`[src/main.rs:]`); internal colons and
// numeric location suffixes stay in the candidate.
const PATH_TRAILING = '.,;!?:';
const LINK_BRACKETS = { '(': ')', '[': ']', '{': '}' };
const PATH_CONTINUATION_MIN_HEAD = 24;

// One pass supplies matching brackets; a bare `name(1).txt` is a filename,
// while `说明(/tmp/a.txt)` hands ownership to the path inside the wrapper.
function linkBracketPairs(text) {
  const stack = [], pairs = new Map();
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (LINK_BRACKETS[ch]) stack.push(i);
    else if (')]}'.includes(ch)) {
      if (stack.length && LINK_BRACKETS[text[stack.at(-1)]] === ch) pairs.set(stack.pop(), i);
      else stack.length = 0;
    }
  }
  return pairs;
}
/* How far a link action may reach back when its first reading does not exist. */
export const PATH_LOOKBACK_MAX = 64;

/* Punctuation and symbols that prose puts BETWEEN things and that no ordinary
   filename puts inside one. Chinese and Japanese write sentences without
   spaces, so these are the only boundary such a line offers: without them
   `src/main.rs。` is read as the filename and `pure.js，然后运行测试。`
   swallows the rest of the sentence. Ranges rather than a list, because an
   enumeration is never finished — 。，、：（）「」 were the first round, and
   ——, →, “”, ～ and ※ each still reached a user inside a "filename" after it.
   Deliberately NOT here: 々 〆 〇 (U+3005..3007) are word characters, and ・ ー
   live in the kana block, so `佐々木.txt` and `データ・ベース.txt` stay whole. */
const isProseSeparator = ch => {
  if (!ch) return false;
  const code = ch.charCodeAt(0);
  return (code >= 0x2010 && code <= 0x205e)     // dashes, curly quotes, …, ※, primes
    || (code >= 0x2190 && code <= 0x21ff)       // arrows
    || (code >= 0x2500 && code <= 0x257f)       // box drawing a TUI puts beside a path
    || (code >= 0x3001 && code <= 0x3004)       // 、。〃〄
    || (code >= 0x3008 && code <= 0x3020)       // 〈〉《》「」『』【】〒〔〕〖〗
    || (code >= 0x3030 && code <= 0x303f)       // 〰 and the rest of the block
    || (code >= 0xff01 && code <= 0xff0f)       // ！＂＃＄％＆＇（）＊＋，－．／
    || (code >= 0xff1a && code <= 0xff20)       // ：；＜＝＞？＠
    || (code >= 0xff3b && code <= 0xff40)       // ［＼］＾＿｀
    || (code >= 0xff5b && code <= 0xff65);      // ｛｜｝～｟｠｡｢｣､･
};

const isSpace = ch => !!ch && ch.trim() === '';
const isAsciiDigit = ch => ch >= '0' && ch <= '9';
const isAsciiLetter = ch => !!ch && ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z'));
const allAsciiDigits = value => !!value && [...value].every(isAsciiDigit);
/* Kana, Han and Hangul: the scripts that write sentences without spaces.
   Their punctuation lives in CJK_PUNCTUATION and is deliberately not here. */
const isCJK = ch => {
  if (!ch) return false;
  const code = ch.charCodeAt(0);
  return (code >= 0x3040 && code <= 0x30ff)      // kana, incl. ・ and ー
    || (code >= 0x3400 && code <= 0x4dbf)        // CJK extension A
    || (code >= 0x4e00 && code <= 0x9fff)        // CJK unified ideographs
    || (code >= 0xac00 && code <= 0xd7af)        // Hangul syllables
    || (code >= 0xf900 && code <= 0xfaff)        // compatibility ideographs
    || (code >= 0xff66 && code <= 0xff9f);       // halfwidth kana
};
/* A CJK character sitting directly against an ASCII LETTER is prose meeting
   a path rather than one name: `修改了src/main.rs` is a sentence that an
   English line would have spelled with a space. The transition has to be a
   LETTER — `日志2024.log` (digit) and `文档/笔记.md` (separator) are single
   names and must survive whole. */
const isScriptBoundary = (prev, ch) =>
  (isCJK(prev) && isAsciiLetter(ch)) || (isAsciiLetter(prev) && isCJK(ch));
const isStartBoundary = ch => !ch || isSpace(ch) || PATH_START_DELIMS.includes(ch)
  || TOKEN_END_DELIMS.includes(ch) || isProseSeparator(ch);
const isTokenEnd = ch => !ch || isSpace(ch) || TOKEN_END_DELIMS.includes(ch)
  || isProseSeparator(ch);

function withoutLineLocation(value) {
  let end = value.length;
  for (let count = 0; count < 2; count++) {
    const colon = value.lastIndexOf(':', end - 1);
    if (colon < 0 || !allAsciiDigits(value.slice(colon + 1, end))) break;
    end = colon;
  }
  return value.slice(0, end);
}

function isDottedNumber(value) {
  let candidate = value;
  if ((candidate[0] === 'v' || candidate[0] === 'V') && isAsciiDigit(candidate[1])) {
    candidate = candidate.slice(1);
  }
  const parts = candidate.split('.');
  return parts.length > 1 && parts.every(allAsciiDigits);
}

export function looksLikeTerminalPathCandidate(value) {
  let raw = String(value);
  if ((raw[0] === '"' || raw[0] === '\'') && raw.lastIndexOf(raw[0]) > 0) {
    const close = raw.lastIndexOf(raw[0]);
    raw = raw.slice(1, close) + raw.slice(close + 1);
  }
  const path = withoutLineLocation(raw);
  if (!path || isDottedNumber(path)) return false;
  const lower = path.toLowerCase();
  if (URL_SCHEMES.some(prefix => lower.startsWith(prefix))) return false;
  if (path === '~' || path.startsWith('~/') || path.startsWith('/')
      || path.startsWith('./') || path.startsWith('../')) return true;
  if (path.includes('/')) return true;
  if (path.startsWith('.') && path.length > 1 && !path.endsWith('.')) return true;
  const dot = path.lastIndexOf('.');
  return dot > 0 && dot < path.length - 1;
}

function urlAt(text, index) {
  /* Only the START takes the script rule: a URL may legitimately carry CJK
     in its own path (`…/wiki/中文`), so nothing ends a URL at a transition. */
  if (!isStartBoundary(text[index - 1])
      && !isScriptBoundary(text[index - 1], text[index])) return null;
  const scheme = URL_SCHEMES.find(prefix =>
    text.slice(index, index + prefix.length).toLowerCase() === prefix);
  if (!scheme) return null;
  let end = index + scheme.length;
  while (end < text.length && !isTokenEnd(text[end])) end++;
  let value = text.slice(index, end);

  // Closing prose punctuation is not part of a URL. Keep balanced brackets,
  // which are valid in paths and queries, but remove unmatched closers.
  const balance = { ')': 0, ']': 0, '}': 0 };
  for (const ch of value) {
    if (LINK_BRACKETS[ch]) balance[LINK_BRACKETS[ch]]++;
    else if (ch in balance) balance[ch]--;
  }
  while (value) {
    const ch = value.at(-1);
    if ('.,;!:'.includes(ch)) value = value.slice(0, -1);
    else if (ch in balance && balance[ch] < 0) {
      balance[ch]++; value = value.slice(0, -1);
    } else break;
  }
  try {
    const parsed = new URL(value);
    if (!URL_SCHEMES.some(prefix => parsed.protocol === prefix.slice(0, -2))
        || !parsed.hostname) return { skipTo: end };
  } catch (_) {
    return { skipTo: end };
  }
  return { kind: 'url', value, index, end: index + value.length };
}

function quotedPathAt(text, index) {
  const quote = text[index];
  if ((quote !== '"' && quote !== '\'') || !isStartBoundary(text[index - 1])) return null;
  const close = text.indexOf(quote, index + 1);
  if (close < 0) return null;
  let end = close + 1;
  for (let locations = 0; locations < 2 && text[end] === ':'; locations++) {
    let digitEnd = end + 1;
    while (isAsciiDigit(text[digitEnd])) digitEnd++;
    if (digitEnd === end + 1) break;
    end = digitEnd;
  }
  const value = text.slice(index, end);
  return looksLikeTerminalPathCandidate(value)
    ? { kind: 'path', value, index, end }
    : null;
}

function unquotedPathAt(text, index, pairs) {
  if ((!isStartBoundary(text[index - 1]) && !isScriptBoundary(text[index - 1], text[index]))
      || isTokenEnd(text[index])
      || PATH_START_DELIMS.includes(text[index])) return null;
  let end = index;
  /* Inside the scan the two directions are not symmetric. Handing prose OVER
     to a path only counts while the token is still a bare word: once a `/` or
     a `.` has been seen the token is a path already, and a script change
     within it belongs to the name — `目录/组合é/…` is one path, and its `é`
     is `e`+U+0301, an ASCII letter pressed against a Han character. Handing
     a path BACK to prose (`src/main.rs的内容`) always ends the token, because
     nothing follows a filename that way except a sentence.
     Residual cost either way: a single component that genuinely runs CJK
     straight into letters, `报告v2.pdf`, is offered as `v2.pdf`. */
  let structural = false;
  const brackets = [];
  while (end < text.length && !isTokenEnd(text[end])
         && !PATH_HARD_END_DELIMS.includes(text[end])) {
    const ch = text[end];
    if (LINK_BRACKETS[ch]) {
      const close = pairs.get(end);
      // An unmatched opener after a path is prose, not a filename bracket.
      // Keeping it swallowed annotations such as `/tmp/work[中文)]`.
      if (close === undefined) break;
      const filenameSuffix = text[close + 1] === '/'
        || text[close + 1] === '.' && !isTokenEnd(text[close + 2])
          && !PATH_START_DELIMS.includes(text[close + 2]);
      if (!structural && !brackets.length && !filenameSuffix) break;
      brackets.push(LINK_BRACKETS[ch]);
    } else if (')]}'.includes(ch)) {
      if (brackets.at(-1) !== ch) break;
      brackets.pop();
    }
    // A bare prose label ends at ':'. Paths keep colons (including :line:col).
    if (ch === ':' && !structural) break;
    if (end > index) {
      const prev = text[end - 1];
      if ((isAsciiLetter(prev) && isCJK(ch))
          || (!structural && isCJK(prev) && isAsciiLetter(ch))) break;
    }
    if (ch === '/' || ch === '.') structural = true;
    end++;
  }
  const rawValue = text.slice(index, end);
  let value = rawValue;
  while (value && PATH_TRAILING.includes(value.at(-1))) value = value.slice(0, -1);
  if (!looksLikeTerminalPathCandidate(value)) return { skipTo: Math.max(index + 1, end) };
  const token = { kind: 'path', value, index, end: index + value.length };
  // If a real filename ends in ':', the filesystem can still choose the
  // original spelling after the punctuation-free reading fails.
  if (rawValue.endsWith(':') && rawValue !== value) token.lookback = rawValue;
  /* The script boundary is a GUESS about where prose ends and a name begins,
     and it is wrong for a name that genuinely runs CJK into letters:
     `报告v2.pdf` is offered as `v2.pdf`, `main日本語.txt` as `日本語.txt`.
     When the token only started because of that guess, carry the prefix it
     cut off so the link ACTIONS can retry with it — the filesystem, not the
     heuristic, then decides which reading was the name. Bounded, so no line
     can grow an unreasonable second candidate. */
  if (!isStartBoundary(text[index - 1])) {
    let start = index;
    const floor = Math.max(0, index - PATH_LOOKBACK_MAX);
    while (start > floor && !isStartBoundary(text[start - 1])
           && !PATH_HARD_END_DELIMS.includes(text[start - 1])) start--;
    if (start < index) token.lookback = text.slice(start, index) + value;
  }
  return token;
}

export function tokenizeTerminalLinks(input) {
  const text = String(input);
  const links = [];
  const pairs = linkBracketPairs(text);
  let index = 0;
  while (index < text.length) {
    const token = urlAt(text, index)
      || quotedPathAt(text, index)
      || unquotedPathAt(text, index, pairs);
    if (token?.skipTo) {
      // Never rescan an already rejected suffix from every inner boundary.
      index = token.skipTo;
    } else if (token) {
      links.push(token);
      index = token.end;
    } else {
      index++;
    }
  }
  return links;
}

export function linkMenuItems(kind) {
  return kind === 'url'
    ? [
      { action: 'url', label: 'Open in browser' },
      { action: 'copy', label: 'Copy URL' },
    ]
    : [
      { action: 'editor', label: 'Open in editor' },
      { action: 'editor-parent', label: 'Open parent folder in editor' },
      { action: 'session-parent', label: 'New session in parent folder' },
      { action: 'reveal', label: 'Reveal in Finder' },
      { action: 'copy', label: 'Copy path' },
    ];
}

/** Link ranges for one xterm buffer line: every tokenized match whose start
 * and end cells are known, restricted to the rows that line covers. */
export function terminalLinkRanges({ matches, positions, lineNo }) {
  const links = [];
  for (const match of matches) {
    const start = positions[match.index];
    const end = positions[match.index + match.value.length - 1];
    if (!start || !end) continue;
    if (lineNo < start.y || lineNo > end.y) continue;
    const link = {
      range: { start: { x: start.x, y: start.y }, end: { x: end.endX, y: end.y } },
      text: match.value, kind: match.kind,
    };
    if (match.lookback) link.lookback = match.lookback;
    links.push(link);
  }
  return links;
}

const MAX_LINK_LOGICAL_ROWS = 32;

function terminalLineFillsWidth(line, cols) {
  if (!line || cols < 1) return false;
  const tail = line.getCell(cols - 1);
  if (!tail) return false;
  if (tail.getChars()?.trim()) return true;
  // The final cell of a width-2 glyph is a zero-width continuation cell.
  if (tail.getWidth() === 0 && cols > 1) {
    const lead = line.getCell(cols - 2);
    return !!lead?.getChars() && lead.getWidth() > 1;
  }
  return false;
}

function terminalRowText(line, cols) {
  let text = '';
  for (let x = 0; x < cols; x++) {
    const cell = line?.getCell(x);
    if (cell && cell.getWidth() !== 0) text += cell.getChars() || ' ';
  }
  return text;
}

/* Agents sometimes insert a real newline and indentation in a long path.
   Join only a path at the end of the previous row with a plausible indented
   continuation. The original grid positions are retained below. */
function pathContinuationIndent(previous, next, cols) {
  if (!previous || !next || next.isWrapped) return 0;
  const before = terminalRowText(previous, cols).trimEnd();
  const after = terminalRowText(next, cols);
  const indent = /^ {2,}/.exec(after)?.[0].length || 0;
  if (!indent) return 0;
  const path = /(?:^|\s)(\S+)$/.exec(before)?.[1];
  if (!path || !looksLikeTerminalPathCandidate(path)) return 0;
  const rest = after.slice(indent);
  const fragment = /^\S+/.exec(rest)?.[0];
  if (!fragment || /^[\[\](){}<>|=,;]/.test(fragment) || /^[-*]\s/.test(rest)) return 0;
  const basename = path.slice(path.lastIndexOf('/') + 1);
  const completeFile = /\.[A-Za-z0-9]{1,12}(?::\d+(?::\d+)?)?$/.test(basename);
  return path.endsWith('/') || /^[.:/-]/.test(fragment)
    || (path.length >= PATH_CONTINUATION_MIN_HEAD && !completeFile
      && (fragment.includes('/') || fragment.includes('.')))
    ? indent : 0;
}

/* Build one visual logical line with UTF-16-offset → terminal-cell mapping,
   using only xterm's public BufferLine/BufferCell APIs. Live output carries
   xterm's isWrapped bit. A tmux attach/history redraw can lose that bit and
   repaint the same wrap as ordinary rows; a nonblank final cell is then a
   continuation signal. Real newline plus indented path continuation is also
   joined. Bound scans so dense TUI output cannot cause unbounded work. */
export function terminalLogicalLine(term, requestedLine) {
  const buffer = term.buffer.active;
  let first = requestedLine - 1;
  while (first > 0 && requestedLine - first < MAX_LINK_LOGICAL_ROWS) {
    const current = buffer.getLine(first);
    const previous = buffer.getLine(first - 1);
    if (!current?.isWrapped && !terminalLineFillsWidth(previous, term.cols)
        && !pathContinuationIndent(previous, current, term.cols)) break;
    first--;
  }
  let last = requestedLine - 1;
  while (last + 1 < buffer.length && last - first + 1 < MAX_LINK_LOGICAL_ROWS) {
    const current = buffer.getLine(last);
    const next = buffer.getLine(last + 1);
    if (!next?.isWrapped && !terminalLineFillsWidth(current, term.cols)
        && !pathContinuationIndent(current, next, term.cols)) break;
    last++;
  }
  let text = '';
  const positions = [];
  let previous = null;
  for (let y = first; y <= last; y++) {
    const line = buffer.getLine(y);
    if (!line) continue;
    const indent = pathContinuationIndent(previous, line, term.cols);
    if (indent) { text = text.trimEnd(); positions.length = text.length; }
    for (let x = indent; x < term.cols; x++) {
      const cell = line.getCell(x);
      if (!cell || cell.getWidth() === 0) continue;
      const chars = cell.getChars() || ' ';
      const pos = { x: x + 1, endX: x + Math.max(1, cell.getWidth()), y: y + 1 };
      text += chars;
      for (let i = 0; i < chars.length; i++) positions.push(pos);
    }
    previous = line;
  }
  const trimmed = text.trimEnd();
  positions.length = trimmed.length;
  return { text: trimmed, positions };
}
