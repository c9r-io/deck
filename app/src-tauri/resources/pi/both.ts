// both.ts — Deck's Pi observer with both capabilities on ONE connection:
// agent run boundaries and the native session this runtime is in.
// The implementation and its contract are `observer.ts`.
import { VERSION } from "@earendil-works/pi-coding-agent";
import { deckPiObserver } from "./observer.ts";

export default deckPiObserver({ signal: true, lifecycle: true }, VERSION);
