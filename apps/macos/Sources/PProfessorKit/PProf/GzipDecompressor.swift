import Foundation
import zlib

/// Decompress gzip-compressed data using zlib.
public func decompressGzip(data: Data) throws -> Data {
    var stream = z_stream()

    // windowBits 15+16 = gzip format
    let initResult = data.withUnsafeBytes { ptr in
        inflateInit2_(&stream, 15 + 16, ZLIB_VERSION, Int32(MemoryLayout<z_stream>.size))
    }
    guard initResult == Z_OK else {
        throw GzipError.initFailed(initResult)
    }
    defer { inflateEnd(&stream) }

    var output = Data()
    let chunkSize = 65536

    try data.withUnsafeBytes { (inputPtr: UnsafeRawBufferPointer) throws in
        guard let inputBase = inputPtr.baseAddress else { return }

        stream.next_in = UnsafeMutablePointer(mutating: inputBase.assumingMemoryBound(to: UInt8.self))
        stream.avail_in = uInt(data.count)

        repeat {
            var chunk = Data(count: chunkSize)
            let result: Int32 = chunk.withUnsafeMutableBytes { outputPtr in
                guard let outputBase = outputPtr.baseAddress else { return Z_BUF_ERROR }
                stream.next_out = outputBase.assumingMemoryBound(to: UInt8.self)
                stream.avail_out = uInt(chunkSize)
                return inflate(&stream, Z_NO_FLUSH)
            }

            guard result == Z_OK || result == Z_STREAM_END else {
                throw GzipError.inflateFailed(result)
            }

            let produced = chunkSize - Int(stream.avail_out)
            output.append(chunk.prefix(produced))

            if result == Z_STREAM_END { break }
        } while stream.avail_out == 0
    }

    return output
}

enum GzipError: Error {
    case initFailed(Int32)
    case inflateFailed(Int32)
}
