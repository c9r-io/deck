// Pane preparation for literal voice input. A bound-target object owns the
// input gate until its own cleanup; a late slice cannot release a newer gate,
// even when both recordings use the same session. Pane identity is rechecked
// after async selection/scroll work. Board and layout access are injected.
export function createVoiceTarget({ getPane, hasCard, cancelSelection, scrollBottom, setDelivering, resetInput }) {
  let owner = null;
  return {
    async prepareTarget(target) {
      const pane = getPane(target.session);
      const visible = () => pane?.attached && getPane(target.session) === pane && hasCard(target.cardId);
      if (!visible()) throw 'target-not-visible';
      owner = target;
      setDelivering(target.session);
      await cancelSelection(pane);
      if (owner !== target || !visible()) throw 'target-not-visible';
      await scrollBottom(target.session);
      if (owner !== target || !visible()) throw 'target-not-visible';
    },
    afterDelivery(target) {
      if (owner !== target) return;
      owner = null;
      setDelivering(null);
      resetInput();
    },
  };
}
