/** Preserve native container structure while profiling each embedded PNG. */
const PNG = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const isPng = (bytes) => bytes.subarray(0, 8).equals(PNG);
async function profileIcns(bytes, profilePng) {
  if (bytes.toString("ascii", 0, 4) !== "icns" || bytes.readUInt32BE(4) !== bytes.length)
    throw new Error("Invalid ICNS header");
  const chunks = [];
  for (let offset = 8; offset < bytes.length;) {
    if (offset + 8 > bytes.length) throw new Error("Truncated ICNS chunk");
    const length = bytes.readUInt32BE(offset + 4);
    if (length < 8 || offset + length > bytes.length) throw new Error("Invalid ICNS chunk length");
    let data = bytes.subarray(offset + 8, offset + length);
    if (isPng(data)) data = await profilePng(data);
    const header = Buffer.alloc(8);
    bytes.copy(header, 0, offset, offset + 4);
    header.writeUInt32BE(data.length + 8, 4);
    chunks.push({ header, data });
    offset += length;
  }
  // The upstream encoder iterates a hash map; stabilise representation order so
  // re-exporting unchanged art does not create noisy binary/hash changes.
  chunks.sort((a, b) => Buffer.compare(a.header.subarray(0, 4), b.header.subarray(0, 4)));
  const body = Buffer.concat(chunks.flatMap(({ header, data }) => [header, data])), header = Buffer.alloc(8);
  header.write("icns"); header.writeUInt32BE(body.length + 8, 4);
  return Buffer.concat([header, body]);
}
async function profileIco(bytes, profilePng) {
  if (bytes.readUInt16LE(0) !== 0 || bytes.readUInt16LE(2) !== 1)
    throw new Error("Invalid ICO header");
  const count = bytes.readUInt16LE(4), directorySize = 6 + count * 16;
  if (!count || directorySize > bytes.length) throw new Error("Invalid ICO directory");
  const header = Buffer.from(bytes.subarray(0, directorySize)), images = [];
  let nextOffset = directorySize;
  for (let i = 0; i < count; i++) {
    const entry = 6 + i * 16, length = bytes.readUInt32LE(entry + 8), offset = bytes.readUInt32LE(entry + 12);
    if (offset < directorySize || !length || offset + length > bytes.length) throw new Error("Invalid ICO image bounds");
    let image = bytes.subarray(offset, offset + length);
    if (isPng(image)) image = await profilePng(image);
    header.writeUInt32LE(image.length, entry + 8); header.writeUInt32LE(nextOffset, entry + 12);
    images.push(image); nextOffset += image.length;
  }
  return Buffer.concat([header, ...images]);
}
module.exports = { profileIcns, profileIco, isPng };
