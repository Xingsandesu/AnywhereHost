import Combine
import Observation
import Sparkle
import SwiftUI

@MainActor @Observable
final class HostUpdater {
    private(set) var canCheck = false
    @ObservationIgnored private let controller: SPUStandardUpdaterController
    @ObservationIgnored private var observation: AnyCancellable?

    init() {
        // The package test runner has no app bundle/feed; it must not start an updater.
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: nil, userDriverDelegate: nil)
        guard Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil else { return }
        observation = controller.updater.publisher(for: \.canCheckForUpdates)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] value in
                MainActor.assumeIsolated { self?.canCheck = value }
            }
        controller.startUpdater()
    }

    func check() { controller.checkForUpdates(nil) }
}

struct HostUpdateButton: View {
    let updater: HostUpdater
    var body: some View {
        Button("检查更新…") { updater.check() }.disabled(!updater.canCheck)
    }
}
