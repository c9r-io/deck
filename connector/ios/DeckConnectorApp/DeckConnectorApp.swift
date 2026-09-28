import DeckConnectorCore
import SwiftUI

@main
struct DeckConnectorApp: App {
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .task {
                    await model.start()
                    #if DEBUG && targetEnvironment(simulator)
                    if case .unpaired = model.connection,
                       ProcessInfo.processInfo.arguments.contains("--deck-diagnostic") {
                        struct Fixture: Decodable { let pairingURI: String }
                        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                        let path = documents.appendingPathComponent("connector-diagnose-fixture.json")
                        if let data = try? Data(contentsOf: path),
                           data.count <= ConnectorLimits.pairingDescriptorBytes * 2,
                           let fixture = try? JSONDecoder().decode(Fixture.self, from: data),
                           let descriptor = try? PairingDescriptor.parse(fixture.pairingURI),
                           let origin = try? HTTPSOrigin(descriptor.origin),
                            origin.host == "127.0.0.1" {
                            await model.pair(descriptor: fixture.pairingURI)
                        } else {
                            model.message = "Diagnostic pairing fixture unavailable."
                        }
                    }
                    #endif
                }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active { Task { await model.refresh() } }
                }
        }
    }
}
