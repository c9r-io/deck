// signal.ts — Deck's Pi observer with the signal capability alone: when an
// agent run starts and when it has settled. Nothing of a conversation is read.
// The implementation and its contract are `observer.ts`.
import { VERSION } from "@earendil-works/pi-coding-agent";
import { deckPiObserver } from "./observer.ts";

export default deckPiObserver({ signal: true, reply: false }, VERSION);
