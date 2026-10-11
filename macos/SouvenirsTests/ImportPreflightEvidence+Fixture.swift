import Foundation
@testable import Souvenirs

extension EngineSupervisor.ImportPreflightEvidence {
    static let fixture = Self(
        representativeFileNames: [
            "2026-10-10 12-14-00 001.jpg",
            "2026-10-10 12-14-00 001.xmp",
            "2026-10-10 14-15-00 001.mov"
        ],
        requiredBytes: 41_000_000_000,
        availableBytes: 500_000_000_000,
        availableSpace: .passed,
        writeProbe: .passed,
        portableNames: .passed,
        destinationConflicts: .passed
    )
}
