// signal.ts — Deck's Pi observer with the signal capability: when an
// agent run starts and when it has settled. No native session id is read.
// The implementation and its contract are `observer.ts`.
import { VERSION } from "@earendil-works/pi-coding-agent";
import { deckPiObserver } from "./observer.ts";

export default deckPiObserver({ signal: true, lifecycle: false }, VERSION);
