// Resume completion is append-only: preserve the typed command byte for byte.
// Recognize a deliberately small shell grammar, never evaluate input. Unknown
// options/compound commands keep ordinary history completion. Only UUID hints
// from the current pane are used; this module never persists them.

const FLAGS = {
  codex: new Set(['--yolo', '--dangerously-bypass-approvals-and-sandbox', '--full-auto', '--search', '--no-alt-screen', '--oss', '--all']),
  claude: new Set(['--dangerously-skip-permissions', '--allow-dangerously-skip-permissions', '--verbose', '--debug', '--chrome', '--no-chrome']),
};
const VALUES = {
  codex: new Set(['--model', '-m', '--profile', '-p', '--config', '-c', '--sandbox', '-s', '--ask-for-approval', '-a', '--cd', '-C', '--add-dir', '--enable', '--disable']),
  claude: new Set(['--model', '--permission-mode', '--effort', '--add-dir', '--settings']),
};

function tokens(line) {
  if (typeof line !== 'string' || line.length > 4096 || /[\x00-\x1f\x7f]/.test(line)) return null;
  const result = [];
  let i = 0;
  while (i < line.length) {
    if (line[i] === ' ') { i++; continue; }
    const start = i;
    let value = '';
    while (i < line.length && line[i] !== ' ') {
      const ch = line[i++];
      if (ch === "'" || ch === '"') {
        const end = line.indexOf(ch, i);
        if (end < 0) return null;
        const quoted = line.slice(i, end);
        if (ch === '"' && /[$`\\]/.test(quoted)) return null;
        value += quoted;
        i = end + 1;
      } else {
        if (/[;&|<>($`\\)#*?{}\[\]!~]/.test(ch)) return null;
        value += ch;
      }
    }
    result.push({ value, start, end: i });
  }
  return result;
}

export function resumeTarget(line) {
  const words = tokens(line);
  const agent = words?.[0]?.value;
  if (!FLAGS[agent] || words[0].start !== 0 || line.slice(0, words[0].end) !== agent) return null;
  let resume = false;
  for (let i = 1; i < words.length; i++) {
    const word = words[i];
    const value = word.value;
    if ((agent === 'codex' && value === 'resume')
        || (agent === 'claude' && ['--resume', '-r'].includes(value))) {
      if (resume) return null;
      resume = true;
    } else if (FLAGS[agent].has(value)) {
      continue;
    } else if (VALUES[agent].has(value.split('=')[0])) {
      if (!value.includes('=')) {
        if (!words[++i] || words[i].value.startsWith('-')) return null;
      }
    } else if (resume && i === words.length - 1 && word.end === line.length
        && /^[a-fA-F0-9-]{1,35}$/.test(value) && line.slice(word.start) === value) {
      return { agent, base: line.slice(0, word.start), partial: value };
    } else {
      return null;
    }
  }
  return resume ? { agent, base: line + (line.endsWith(' ') ? '' : ' '), partial: '' } : null;
}

export function resumeCommands(line, hints) {
  const target = resumeTarget(line);
  if (!target) return [];
  return [...new Set(hints.filter(h => h.agent === target.agent
    && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(h.id)
    && h.id.toLowerCase().startsWith(target.partial.toLowerCase()))
    .map(h => target.base + target.partial + h.id.slice(target.partial.length)))].slice(0, 6);
}

// One request per pane/input episode. Invalidation revokes pending results,
// including an A→B→A focus switch. Owners are pane objects, not reusable names.
export function createResumeCache(read, changed) {
  let entries = new WeakMap();
  return {
    get(owner) {
      let entry = entries.get(owner);
      if (!entry) {
        entry = { hints: [] };
        entries.set(owner, entry);
        Promise.resolve().then(() => read(owner)).then(hints => {
          if (entries.get(owner) !== entry) return;
          entry.hints = hints;
          changed(owner);
        }).catch(() => {});
      }
      return entry.hints;
    },
    reset() { entries = new WeakMap(); },
  };
}
