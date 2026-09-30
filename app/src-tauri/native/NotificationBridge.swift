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
private var permissionRequests = 0
private var permissionError = 0
private var permissionCompleted = false
// Numeric observation only; the debug carrier reads the real request result.
func deckNotifyPermissionDiagnostics() -> [String: Any] {
    statusLock.lock(); defer { statusLock.unlock() }
    return ["requests": permissionRequests, "completed": permissionCompleted, "errorCode": permissionError]
}

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
        if reminderIdentifierOk(id) {
            let kind: Int32
            switch response.actionIdentifier {
            case UNNotificationDefaultActionIdentifier, "reminder-open": kind = 1
            case "reminder-snooze": kind = 2
            default: completionHandler(); return
            }
            id.withCString { reminderAction?($0, kind, UInt64(Date().timeIntervalSince1970 * 1000)) }
        } else if deckNotifyIdentifierOk(id), let callback = openCallback {
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
    statusLock.lock(); permissionRequests += 1; statusLock.unlock()
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { granted, error in
        statusLock.lock(); permissionCompleted = true; permissionError = (error as NSError?)?.code ?? 0; statusLock.unlock()
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

// Reminder identifiers are a separate, closed namespace. No Agent removal
// operation accepts these identifiers. Notes never reach notification content.
private func reminderIdentifierOk(_ id: String) -> Bool {
    let parts = id.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count == 5, parts[0] == "deck", parts[1] == "reminder",
          !parts[2].isEmpty, parts[2].count <= 1024, parts[2].count % 2 == 0,
          parts[3].count == 32, let revision = UInt32(parts[4]), revision > 0, revision <= 1000000 else { return false }
    return (parts[2] + parts[3]).allSatisfy { "0123456789abcdef".contains($0) }
}
public typealias DeckReminderAction = @convention(c) (UnsafePointer<CChar>, Int32, UInt64) -> Void
public typealias DeckReminderResult = @convention(c) (UnsafePointer<CChar>, Int32) -> Void
private var reminderAction: DeckReminderAction?
private var reminderResult: DeckReminderResult?
private let reminderQueue = DispatchQueue(label: "deck.reminder.projection")
private let reminderLock = NSLock()
private var reminderDesired = Set<String>()
private func reminderCurrent(_ id: String) -> Bool {
    reminderLock.lock(); defer { reminderLock.unlock() }
    return reminderDesired.contains(id)
}
@_cdecl("deck_reminder_init")
public func deckReminderInit(_ action: @escaping DeckReminderAction, _ result: @escaping DeckReminderResult) {
    guard deckNotifyBundled() else { return }
    reminderAction = action; reminderResult = result
    func category(_ chinese: Bool) -> UNNotificationCategory {
        let open = UNNotificationAction(identifier: "reminder-open", title: chinese ? "打开卡片" : "Open card", options: [.foreground])
        let snooze = UNNotificationAction(identifier: "reminder-snooze", title: chinese ? "推迟1小时" : "Remind in 1 hour", options: [])
        return UNNotificationCategory(identifier: chinese ? "deck-reminder-zh" : "deck-reminder-en", actions: [open, snooze], intentIdentifiers: [], options: [])
    }
    UNUserNotificationCenter.current().setNotificationCategories([category(false), category(true)])
    NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: nil) { _ in
        "".withCString { reminderResult?($0, 3) }
    }
    NotificationCenter.default.addObserver(forName: NSNotification.Name.NSSystemClockDidChange, object: nil, queue: nil) { _ in
        "".withCString { reminderResult?($0, 3) }
    }
}

@_cdecl("deck_reminder_project")
public func deckReminderProject(_ raw: UnsafePointer<CChar>) {
    guard deckNotifyBundled(), let data = String(cString: raw).data(using: .utf8),
          let rows = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] else { return }
    let ids = Set(rows.compactMap { $0["identifier"] as? String }.filter(reminderIdentifierOk))
    reminderLock.lock(); reminderDesired = ids; reminderLock.unlock()
    reminderQueue.async {
        let center = UNUserNotificationCenter.current()
        var pending = [UNNotificationRequest](); var delivered = [UNNotification]()
        let read = DispatchGroup()
        read.enter(); center.getNotificationSettings { settings in setStatus(deckNotifyStatusCode(settings.authorizationStatus)); read.leave() }
        read.enter(); center.getPendingNotificationRequests { pending = $0; read.leave() }
        read.enter(); center.getDeliveredNotifications { delivered = $0; read.leave() }
        guard read.wait(timeout: .now() + 5) == .success else {
            for id in ids { id.withCString { reminderResult?($0, 2) } }; return
        }
        let obsolete = Set(pending.map { $0.identifier } + delivered.map { $0.request.identifier })
            .filter { reminderIdentifierOk($0) && !ids.contains($0) && !reminderCurrent($0) }
        center.removePendingNotificationRequests(withIdentifiers: Array(obsolete))
        center.removeDeliveredNotifications(withIdentifiers: Array(obsolete))
        let deliveredIDs = Set(delivered.map { $0.request.identifier })
        for row in rows {
            guard let id = row["identifier"] as? String, reminderIdentifierOk(id), reminderCurrent(id),
                  let dueAt = row["dueAt"] as? Double else { continue }
            if deliveredIDs.contains(id) { id.withCString { reminderResult?($0, 1) }; continue }
            // Recovery never re-arms a missed/past-due request. The Board
            // retains due attention; scheduling is not a banner guarantee.
            guard row["due"] as? Bool != true, dueAt > Date().timeIntervalSince1970 * 1000 else { continue }
            statusLock.lock(); let authorization = cachedStatus; statusLock.unlock()
            guard authorization == 3 || authorization == 4 else { id.withCString { reminderResult?($0, 2) }; continue }
            let content = UNMutableNotificationContent()
            content.title = row["title"] as? String ?? "Deck"
            let project = row["project"] as? String ?? ""
            let locale = row["locale"] as? String ?? "system"
            let chinese = locale == "zh-Hans" || locale == "system" && Locale.preferredLanguages.first?.hasPrefix("zh") == true
            let phrase = chinese ? "回到此卡片" : "Return to this card"
            content.body = project.isEmpty ? phrase : "\(project) — \(phrase)"
            content.categoryIdentifier = chinese ? "deck-reminder-zh" : "deck-reminder-en"
            if row["sound"] as? Bool == true { content.sound = .default }
            var calendar = Calendar(identifier: .gregorian); calendar.timeZone = TimeZone(secondsFromGMT: 0)!
            var components = calendar.dateComponents([.year, .month, .day, .hour, .minute, .second], from: Date(timeIntervalSince1970: dueAt / 1000))
            components.calendar = calendar; components.timeZone = calendar.timeZone
            if let existing = pending.first(where: { $0.identifier == id }),
               existing.content.title == content.title, existing.content.body == content.body,
               existing.content.categoryIdentifier == content.categoryIdentifier,
               (existing.content.sound != nil) == (content.sound != nil) {
                id.withCString { reminderResult?($0, 1) }; continue
            }
            let request = UNNotificationRequest(identifier: id, content: content, trigger: UNCalendarNotificationTrigger(dateMatching: components, repeats: false))
            let added = DispatchSemaphore(value: 0)
            center.add(request) { error in
                if !reminderCurrent(id) {
                    center.removePendingNotificationRequests(withIdentifiers: [id]); center.removeDeliveredNotifications(withIdentifiers: [id])
                } else { id.withCString { reminderResult?($0, error == nil ? 1 : 2) } }
                added.signal()
            }
            if added.wait(timeout: .now() + 5) != .success { id.withCString { reminderResult?($0, 2) } }
        }
    }
}

// Calendar validation uses Foundation only and is safe outside a bundle.
@_cdecl("deck_reminder_zone_valid")
public func deckReminderZoneValid(_ raw: UnsafePointer<CChar>) -> Int32 {
    return TimeZone(identifier: String(cString: raw)) == nil ? 0 : 1
}
