/**
 * attachments.ts 冒烟测试：自造 docx/xlsx（store 方式 zip）、手写 PDF、txt，
 * 验证分类、提取与错误块。node/bun 均可运行：
 *   bun run scripts/attachments-smoke.ts
 */
import { deflateRawSync } from "node:zlib";
import { parseAttachments } from "../src/attachments.js";

/** 最小 zip 写入（method 8 deflate，与真实 office 文档一致） */
function makeZip(files: Record<string, string>): Buffer {
  const chunks: Buffer[] = [];
  const central: Buffer[] = [];
  let offset = 0;
  for (const [name, content] of Object.entries(files)) {
    const nameBuf = Buffer.from(name, "utf8");
    const data = deflateRawSync(Buffer.from(content, "utf8"));
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(8, 8);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(Buffer.byteLength(content), 22);
    local.writeUInt16LE(nameBuf.length, 26);
    chunks.push(local, nameBuf, data);
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0);
    cd.writeUInt16LE(20, 4);
    cd.writeUInt16LE(8, 10);
    cd.writeUInt32LE(data.length, 20);
    cd.writeUInt32LE(Buffer.byteLength(content), 24);
    cd.writeUInt16LE(nameBuf.length, 28);
    cd.writeUInt32LE(offset, 42);
    central.push(cd, nameBuf);
    offset += 30 + nameBuf.length + data.length;
  }
  const centralBuf = Buffer.concat(central);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(Object.keys(files).length, 10);
  eocd.writeUInt32LE(centralBuf.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...chunks, centralBuf, eocd]);
}

/** 手写单页 PDF（WinAnsi 简单文本，pdf.js 可解析） */
function makePdf(lines: string[]): Buffer {
  const body = lines.map((l) => `(${l.replace(/([()\\])/g, "\\$1")}) Tj T*`).join("\n");
  const content = `BT\n/F1 12 Tf\n14 TL\n72 720 Td\n${body}\nET`;
  const objs = [
    "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    `4 0 obj\n<< /Length ${Buffer.byteLength(content)} >>\nstream\n${content}\nendstream\nendobj\n`,
    "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
  ];
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  for (const o of objs) {
    offsets.push(Buffer.byteLength(out));
    out += o;
  }
  const xrefAt = Buffer.byteLength(out);
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`;
  for (const off of offsets) out += `${String(off).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xrefAt}\n%%EOF`;
  return Buffer.from(out, "utf8");
}

function b64(s: string | Buffer): string {
  return Buffer.from(s as Buffer).toString("base64");
}

const docx = makeZip({
  "word/document.xml": `<?xml version="1.0" encoding="UTF-8"?><w:document><w:body><w:p><w:r><w:t>第一段：项目进度总结</w:t></w:r></w:p><w:p><w:r><w:t>Second &amp; line with &lt;tag&gt; </w:t><w:tab/><w:t>after tab</w:t></w:r></w:p></w:body></w:document>`,
});
const xlsx = makeZip({
  "xl/sharedStrings.xml": `<?xml version="1.0"?><sst><si><t>名称</t></si><si><t>Alpha</t></si></sst>`,
  "xl/worksheets/sheet1.xml": `<?xml version="1.0"?><worksheet><sheetData><row r="1"><c t="s"><v>0</v></c><c t="s"><v>1</v></c><c><v>42</v></c></row><row r="2"><c><v>1.5</v></c><c t="inlineStr"><is><t>内联文本</t></is></c></row></sheetData></worksheet>`,
});
const pptx = makeZip({
  "ppt/slides/slide1.xml": `<?xml version="1.0"?><p:sld><p:cSld><p:spTree><a:p><a:r><a:t>标题页</a:t></a:r></a:p><a:p><a:r><a:t>要点一</a:t></a:r></a:p></p:spTree></p:cSld></p:sld>`,
});
const pdf = makePdf(["Hello PDF", "Second line (parens)"]);

const cases = [
  { name: "报告.docx", size: docx.length, data: b64(docx) },
  { name: "表格.xlsx", size: xlsx.length, data: b64(xlsx) },
  { name: "slides.pptx", size: pptx.length, data: b64(pptx) },
  { name: "notes.txt", size: 0, data: b64("plain text attachment") },
  { name: "doc.pdf", size: pdf.length, data: b64(pdf) },
  { name: "pic.png", size: 4, data: b64("fake"), mime_type: "image/png" },
  { name: "legacy.doc", size: 10, data: b64("0123456789") },
  { name: "unknown.bin", size: 4, data: b64([0, 1, 2, 3].toString()) },
];

const r = await parseAttachments(cases as never);
for (const b of r.blocks) console.log("=== BLOCK ===\n" + b.slice(0, 400) + (b.length > 400 ? "…(截断展示)" : "") + "\n");
console.log("images:", r.images.length, JSON.stringify(r.images));
// 超过 MAX_FILES=6 的部分（legacy.doc / unknown.bin）应被丢弃
console.log(
  r.blocks.length === 5 &&
    r.images.length === 1 &&
    r.blocks[0]!.includes("项目进度总结") &&
    r.blocks[0]!.includes("<tag>") &&
    r.blocks[0]!.includes("after tab") &&
    r.blocks[1]!.includes("名称\tAlpha\t42") &&
    r.blocks[1]!.includes("内联文本") &&
    r.blocks[2]!.includes("标题页") &&
    r.blocks[3]!.includes("plain text attachment") &&
    r.blocks[4]!.includes("Hello PDF") &&
    r.blocks[4]!.includes("Second line (parens)") &&
    !r.blocks.some((b) => b.includes("legacy.doc"))
    ? "SMOKE OK"
    : "SMOKE MISMATCH",
);
