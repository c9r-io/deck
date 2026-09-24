// In-process macOS user notifications for deck's away notifications
// (notify.rs owns the policy: what is posted, when, and the Dock badge).
// This file only talks to UNUserNotificationCenter: request authorization,
// post one notification per tmux session identifier, remove it, and hand a
// click back to Rust as that identifier. Nothing here logs, spawns, reads
// files or opens a socket; strings cross the bridge one way (Rust → system)
// except the clicked identifier, which is deck's own session name.
// Every entry is a no-op outside a real .app bundle (unit tests, a bare
// binary): UNUserNotificationCenter aborts the process without a bundle.
import Foundation
import AppKit
import UserNotifications

public typealias DeckNotifyOpenCallback = @convention(c) (UnsafePointer<CChar>) -> Void

/// Closed status codes shared with notify.rs: 0 unsupported (no bundle),
/// 1 not determined, 2 denied, 3 authorized, 4 provisional.
func deckNotifyStatusCode(_ status: UNAuthorizationStatus) -> Int32 {
    switch status {
    case .notDetermined: return 1
    case .denied: return 2
    case .authorized: return 3
    case .provisional: return 4
    default: return 3 // .ephemeral and future cases can present
    }
}

/// A notification identifier is a deck tmux session name: the closed
/// alphabet tmux.rs validates, bounded, so nothing else is ever addressed.
func deckNotifyIdentifierOk(_ id: String) -> Bool {
    if id.isEmpty || id.utf8.count > 64 { return false }
    for scalar in id.unicodeScalars {
        switch scalar {
        case "a"..."z", "A"..."Z", "0"..."9", "-", "_": continue
        default: return false
        }
    }
    return true
}

/// Only a launched .app bundle may use the notification center.
func deckNotifyBundled() -> Bool {
    Bundle.main.bundleIdentifier != nil && Bundle.main.bundleURL.pathExtension == "app"
}

private let statusLock = NSLock()
private var cachedStatus: Int32 = 0
private var openCallback: DeckNotifyOpenCallback?

private func setStatus(_ code: Int32) {
    statusLock.lock()
    cachedStatus = code
    statusLock.unlock()
}

private func refreshStatus() {
    UNUserNotificationCenter.current().getNotificationSettings { settings in
        setStatus(deckNotifyStatusCode(settings.authorizationStatus))
    }
}

private final class DeckNotifyDelegate: NSObject, UNUserNotificationCenterDelegate {
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let id = response.notification.request.identifier
        if deckNotifyIdentifierOk(id), let callback = openCallback {
            id.withCString { callback($0) }
        }
        completionHandler()
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        // deck only posts while its window is not focused; if focus returned
        // before delivery the banner is still shown rather than dropped.
        if #available(macOS 11.0, *) {
            completionHandler([.banner, .list, .sound])
        } else {
            completionHandler([.alert, .sound])
        }
    }
}

private let delegate = DeckNotifyDelegate()

/// Installs the click delegate and reads the current authorization once.
/// Returns 0 outside a bundle (every later call is then a no-op).
@_cdecl("deck_notify_init")
public func deckNotifyInit(_ callback: @escaping DeckNotifyOpenCallback) -> Int32 {
    guard deckNotifyBundled() else { return 0 }
    openCallback = callback
    UNUserNotificationCenter.current().delegate = delegate
    // The first settings query answers asynchronously; until it does, a
    // bundled app reads as "not determined", never as "unsupported".
    setStatus(1)
    refreshStatus()
    return 1
}

/// Asks the user once (the system dialog); the cached status follows.
@_cdecl("deck_notify_request")
public func deckNotifyRequest() {
    guard deckNotifyBundled() else { return }
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { granted, _ in
        setStatus(granted ? 3 : 2)
        refreshStatus()
    }
}

/// The last known authorization status (see the code table above).
@_cdecl("deck_notify_status")
public func deckNotifyStatus() -> Int32 {
    guard deckNotifyBundled() else { return 0 }
    refreshStatus()
    statusLock.lock()
    let code = cachedStatus
    statusLock.unlock()
    return code
}

/// Posts or replaces the notification for one session. Title and body are
/// what Rust passed (a card title and a closed phrase); nothing is added.
/// Returns 1 when handed to the system, 0 when refused here.
@_cdecl("deck_notify_post")
public func deckNotifyPost(_ id: UnsafePointer<CChar>, _ title: UnsafePointer<CChar>,
                           _ body: UnsafePointer<CChar>, _ sound: Int32) -> Int32 {
    guard deckNotifyBundled() else { return 0 }
    let identifier = String(cString: id)
    guard deckNotifyIdentifierOk(identifier) else { return 0 }
    let content = UNMutableNotificationContent()
    content.title = String(cString: title)
    content.body = String(cString: body)
    if sound != 0 {
        content.sound = .default
    }
    let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
    UNUserNotificationCenter.current().add(request) { _ in }
    return 1
}

/// Withdraws one session's notification from the banner and the list.
@_cdecl("deck_notify_remove")
public func deckNotifyRemove(_ id: UnsafePointer<CChar>) {
    guard deckNotifyBundled() else { return }
    let identifier = String(cString: id)
    guard deckNotifyIdentifierOk(identifier) else { return }
    let center = UNUserNotificationCenter.current()
    center.removePendingNotificationRequests(withIdentifiers: [identifier])
    center.removeDeliveredNotifications(withIdentifiers: [identifier])
}
