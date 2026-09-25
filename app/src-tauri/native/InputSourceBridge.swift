// The system Text Input Sources API is the sole owner of the selected source.
// The observer callback carries no borrowed pointers across threads. The
// snapshot's allocated string is released by deck_input_source_free_name.
import Foundation
import Carbon
import ImageIO

public typealias DeckInputSourceCallback = @convention(c) () -> Void

private var inputSourceCallback: DeckInputSourceCallback?
private var inputSourceObserving = false

private struct InputSourceSnapshot: Encodable {
    let name: String?
    let icon: String?
}

private func inputSourceIcon(_ source: TISInputSource) -> String? {
    guard let property = TISGetInputSourceProperty(source, kTISPropertyIconImageURL) else { return nil }
    let url = (Unmanaged<CFURL>.fromOpaque(property).takeUnretainedValue() as URL).absoluteURL
    guard let size = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber,
          size.intValue <= 1_048_576,
          let imageSource = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
    let thumbnailOptions: [CFString: Any] = [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceThumbnailMaxPixelSize: 32,
        kCGImageSourceShouldCache: false,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(imageSource, 0, thumbnailOptions as CFDictionary) else { return nil }
    let data = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil) else { return nil }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { return nil }
    return "data:image/png;base64," + (data as Data).base64EncodedString()
}

private func currentInputSourceSnapshot() -> InputSourceSnapshot {
    guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else {
        return InputSourceSnapshot(name: nil, icon: nil)
    }
    let name = TISGetInputSourceProperty(source, kTISPropertyLocalizedName)
        .flatMap { Unmanaged<AnyObject>.fromOpaque($0).takeUnretainedValue() as? String }
    return InputSourceSnapshot(name: name, icon: inputSourceIcon(source))
}

private func inputSourceChanged(_ center: CFNotificationCenter?,
                                _ observer: UnsafeMutableRawPointer?,
                                _ name: CFNotificationName?,
                                _ object: UnsafeRawPointer?,
                                _ userInfo: CFDictionary?) {
    inputSourceCallback?()
}

@_cdecl("deck_input_source_init")
public func deck_input_source_init(_ callback: @escaping DeckInputSourceCallback) -> Int32 {
    if inputSourceObserving { return 1 }
    inputSourceCallback = callback
    CFNotificationCenterAddObserver(CFNotificationCenterGetDistributedCenter(), nil,
                                    inputSourceChanged, kTISNotifySelectedKeyboardInputSourceChanged,
                                    nil, .deliverImmediately)
    inputSourceObserving = true
    return 1
}

@_cdecl("deck_input_source_copy_snapshot")
public func deck_input_source_copy_snapshot() -> UnsafeMutablePointer<CChar>? {
    guard let value = try? JSONEncoder().encode(currentInputSourceSnapshot()) else { return nil }
    let bytes = value.map { CChar(bitPattern: $0) } + [0]
    let pointer = UnsafeMutablePointer<CChar>.allocate(capacity: bytes.count)
    bytes.withUnsafeBufferPointer { buffer in
        pointer.initialize(from: buffer.baseAddress!, count: buffer.count)
    }
    return pointer
}

@_cdecl("deck_input_source_free_snapshot")
public func deck_input_source_free_snapshot(_ pointer: UnsafeMutablePointer<CChar>?) {
    pointer?.deallocate()
}
