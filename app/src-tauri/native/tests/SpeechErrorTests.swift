// Exercise the production classifier without microphone access or model loads.
import Foundation

@main struct SpeechErrorTests {
    static func main() {
        let disabled = NSError(domain: "kLSRErrorDomain", code: 201)
        precondition(deckSpeechErrorCode(disabled) == "dictation-disabled")
        let wrapped = NSError(domain: "kAFAssistantErrorDomain", code: 203,
                              userInfo: [NSUnderlyingErrorKey: disabled])
        precondition(deckSpeechErrorCode(wrapped) == "dictation-disabled")
        for error in [
            NSError(domain: "unrelated", code: 201),
            NSError(domain: "kLSRErrorDomain", code: 999),
            NSError(domain: "unrelated", code: 0,
                    userInfo: [NSLocalizedDescriptionKey: "Siri and Dictation are disabled"]),
            NSError(domain: "unrelated", code: 0, userInfo: [NSUnderlyingErrorKey: "private text"]),
        ] {
            precondition(deckSpeechErrorCode(error) == "recognition-failed")
        }
        var deep = disabled
        for _ in 0..<16 {
            deep = NSError(domain: "wrapper", code: 0, userInfo: [NSUnderlyingErrorKey: deep])
        }
        precondition(deckSpeechErrorCode(deep) == "recognition-failed")
    }
}
