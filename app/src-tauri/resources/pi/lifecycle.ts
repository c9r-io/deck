// lifecycle.ts — Deck's Pi observer with the lifecycle capability alone:
// which native session this runtime is in. No agent run is observed.
// The implementation and its contract are `observer.ts`.
import { VERSION } from "@earendil-works/pi-coding-agent";
import { deckPiObserver } from "./observer.ts";

export default deckPiObserver({ signal: false, lifecycle: true }, VERSION);
