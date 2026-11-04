import AppKit
import SwiftUI
import CoreImage.CIFilterBuiltins

struct ConnectionSettings: View {
    @Bindable var store: HostStore
    @State private var link: String?
    @State private var qr: NSImage?
    @State private var knownDevices = Set<String>()
    @State private var generating = false
    @State private var expires: Date?
    var body: some View {
        HostSheet(title: "连接设备") {
            ScrollView {
                VStack(spacing: 20) {
                    ZStack {
                        if let qr, let expires, expires > .now {
                            Image(nsImage: qr).interpolation(.none).resizable().scaledToFit()
                                .frame(width: 164, height: 164).padding(12).background(.white, in: .rect(cornerRadius: 16))
                                .accessibilityLabel("手机扫描此二维码配对")
                        } else { Image(systemName: "iphone.and.arrow.forward").font(.system(size: 48)) }
                    }
                    Text("用 iOS 客户端扫描二维码").font(.title3)
                    Text("二维码两分钟内有效，仅能使用一次。请使用自己的手机扫描。").foregroundStyle(.secondary)
                    HStack {
                        Button("生成配对二维码") { Task { await generate() } }.disabled(store.busy || generating || !store.serviceAvailable)
                        if let link { Button("复制链接") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(link, forType: .string) } }
                    }
                    Divider()
                    HostPairedDevices(store: store)
                    if let error = store.error { Text(error).foregroundStyle(.red) }
                }.padding(24)
            }
        }.frame(width: 540, height: 700)
        .task {
            await loadDevices()
            await generate()
            while !Task.isCancelled {
                await loadDevices()
                if let expires, expires <= .now { link = nil; qr = nil }
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
    }
    private func generate() async {
        guard !generating else { return }
        generating = true
        defer { generating = false }
        do {
            guard let client = store.client else { throw HostFailure(message: "主机服务未启动") }
            let value = try await client.pairingLink()
            let filter = CIFilter.qrCodeGenerator(); filter.message = Data(value.utf8); filter.correctionLevel = "M"
            guard let output = filter.outputImage, let cg = CIContext().createCGImage(output, from: output.extent) else { throw HostFailure(message: "二维码生成失败") }
            qr = NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height)); link = value; expires = .now.addingTimeInterval(120)
        } catch { store.error = error.localizedDescription }
    }
    private func loadDevices() async {
        await store.refresh()
        if !store.serviceAvailable { link = nil; qr = nil; expires = nil; return }
        let next = Set(store.devices.map(\.id))
        if !next.subtracting(knownDevices).isEmpty { link = nil; qr = nil; expires = nil }
        knownDevices = next
    }
}
