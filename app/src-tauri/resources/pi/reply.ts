// reply.ts — Deck's Pi observer with agent run boundaries and each run's
// final message: the user's "forward Pi's final message" switch.
// The implementation and its contract are `observer.ts`.
import { VERSION } from "@earendil-works/pi-coding-agent";
import { deckPiObserver } from "./observer.ts";

export default deckPiObserver({ signal: true, reply: true }, VERSION);
