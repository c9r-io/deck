// Presentation gate for the two session-header MCP actions. Backend authority
// is checked separately; this module only keeps the visible UI fail-closed.
export function createMcpSessionUiGate() {
  let epoch = 0;
  let confirmedCardId = null;
  return {
    begin() { confirmedCardId = null; return ++epoch; },
    accept(requestEpoch, cardId, currentCardId, status) {
      if (requestEpoch !== epoch || cardId !== currentCardId || status?.managed !== true || status.stale === true) return false;
      confirmedCardId = cardId;
      return true;
    },
    mayAct(cardId) { return confirmedCardId !== null && confirmedCardId === cardId; },
  };
}

export function resetMcpSessionControls(control, grant) {
  for (const button of [control, grant]) {
    button.hidden = true;
    button.disabled = true;
    delete button.dataset.human;
    delete button.dataset.active;
    button.textContent = '';
    button.title = '';
  }
}
