/**
 * 文档附件解析：把用户上传的附件（base64）提取为纯文本，注入 prompt。
 *
 * pi SDK 0.80 的 prompt 只支持 text/image 内容，agent 的 read 工具也读不了
 * pdf/docx 等二进制格式，所以文档必须在 host 侧先提取成文本。图片附件直接
 * 路由到 prompt 的 images 数组，与「添加图片」同一通道。
 *
 * 提取结果以 <attachment> 块追加在用户文本之后（随会话落盘，历史回放可见）。
 * UI 端 MessageItem 按同名规则解析并折叠为 chip，两侧格式需保持一致：
 *
 *   <attachment name="报告.pdf" size="12345">
 *   提取出的正文…
 *   </attachment>
 */
import { inflateRawSync } from "node:zlib";
import { extractText } from "unpdf";
import { convertToPng, formatDimensionNote, resizeImage } from "@earendil-works/pi-coding-agent";

export interface IncomingAttachment {
  /** 文件名（含扩展名，扩展名是类型识别的主要依据） */
  name: string;
  mime_type?: string;
  /** 原始字节数，仅用于展示与校验 */
  size?: number;
  /** base64，不带 data: 前缀 */
  data: string;
}

export interface ParsedAttachments {
  /** 从附件中识别出的图片，直接并入 prompt images */
  images: Array<{ type: "image"; data: string; mimeType: string }>;
  /** <attachment> 文本块，按序追加在用户文本后 */
  blocks: string[];
  /** 大图缩放的尺寸注记（给模型说明坐标映射），按序追加在用户文本后 */
  notes: string[];
}

const MAX_FILES = 6;
const MAX_INPUT_BYTES = 25 * 1024 * 1024;
const MAX_CHARS_PER_FILE = 60_000;
const MAX_TOTAL_CHARS = 150_000;

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
const TEXT_EXTS = new Set([
  "txt", "md", "markdown", "csv", "tsv", "json", "yaml", "yml", "toml", "ini", "cfg", "conf",
  "env", "log", "xml", "html", "htm", "svg", "css", "scss", "less", "vue", "svelte",
  "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "rb", "php", "java", "kt", "swift", "scala",
  "c", "h", "cpp", "hpp", "cc", "cs", "go", "rs", "sql", "sh", "bash", "zsh", "bat", "cmd",
  "ps1", "lua", "r", "proto", "graphql", "properties", "srt", "vtt", "gitignore", "lock",
]);

function extOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

function fmtSize(n: number): string {
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
  return `${n} B`;
}

function block(name: string, size: number, body: string): string {
  return `<attachment name="${name}" size="${size}">\n${body}\n</attachment>`;
}

function errorBlock(name: string, size: number, reason: string): string {
  return block(name, size, `[附件未能读取：${reason}]`);
}

function capChars(text: string, max = MAX_CHARS_PER_FILE): string {
  if (text.length <= max) return text;
  return `${text.slice(0, max)}\n\n[内容过长，已截断：共 ${text.length} 字符，仅保留前 ${max} 字符]`;
}

// ---------------------------------------------------------------- pdf

async function pdfToText(buf: Buffer): Promise<string> {
  const { text } = await extractText(new Uint8Array(buf), { mergePages: true });
  const t = (Array.isArray(text) ? text.join("\n\n") : text).replace(/\n{3,}/g, "\n\n").trim();
  if (!t) throw new Error("未提取到文本（可能是扫描件或加密 PDF）");
  return t;
}

// ---------------------------------------------------------------- ooxml（zip + xml，零依赖）

/** 最小 zip 读取：解析中央目录，返回 name → 解压后的内容（不支持 zip64） */
function readZipEntries(buf: Buffer): Map<string, Buffer> {
  let eocd = buf.length - 22;
  const min = Math.max(0, buf.length - 22 - 65536);
  while (eocd >= min && buf.readUInt32LE(eocd) !== 0x06054b50) eocd--;
  if (eocd < min) throw new Error("不是有效的 zip 文档");
  const count = buf.readUInt16LE(eocd + 10);
  let off = buf.readUInt32LE(eocd + 16);
  const entries = new Map<string, Buffer>();
  for (let n = 0; n < count && off + 46 <= buf.length; n++) {
    if (buf.readUInt32LE(off) !== 0x02014b50) break;
    const method = buf.readUInt16LE(off + 10);
    const csize = buf.readUInt32LE(off + 20);
    const nameLen = buf.readUInt16LE(off + 28);
    const extraLen = buf.readUInt16LE(off + 30);
    const commentLen = buf.readUInt16LE(off + 32);
    const lho = buf.readUInt32LE(off + 42);
    const name = buf.subarray(off + 46, off + 46 + nameLen).toString("utf8");
    if (buf.readUInt32LE(lho) === 0x04034b50 && csize > 0) {
      const dataStart = lho + 30 + buf.readUInt16LE(lho + 26) + buf.readUInt16LE(lho + 28);
      try {
        const raw = buf.subarray(dataStart, dataStart + csize);
        entries.set(name, method === 0 ? Buffer.from(raw) : inflateRawSync(raw));
      } catch {
        // 单个坏条目不影响其余内容
      }
    }
    off += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

function decodeXmlEntities(s: string): string {
  return s.replace(/&(amp|lt|gt|quot|apos|#x?[0-9a-fA-F]+);/g, (_, e: string) => {
    if (e === "amp") return "&";
    if (e === "lt") return "<";
    if (e === "gt") return ">";
    if (e === "quot") return '"';
    if (e === "apos") return "'";
    const body = e[1] === "x" || e[1] === "X" ? e.slice(2) : e.slice(1);
    const code = parseInt(body, e[1] === "x" || e[1] === "X" ? 16 : 10);
    return Number.isFinite(code) && code > 0 ? String.fromCodePoint(code) : _;
  });
}

/** 去掉所有标签，段落/换行标记提前替换为换行 */
function xmlToText(xml: string, breaks: RegExp[]): string {
  let s = xml;
  for (const re of breaks) s = s.replace(re, "\n");
  return decodeXmlEntities(s.replace(/<[^>]+>/g, ""))
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function docxToText(entries: Map<string, Buffer>): string {
  const xml = entries.get("word/document.xml");
  if (!xml) throw new Error("缺少 word/document.xml");
  const text = xmlToText(xml.toString("utf8"), [/<\/w:p>/g, /<w:tab\b[^>]*\/>/g, /<w:br\b[^>]*\/>/g]);
  if (!text) throw new Error("文档正文为空");
  return text;
}

/** 共享字符串表：<si> 内的多个 <r><t> run 拼接为一个字符串 */
function parseSharedStrings(xml: string): string[] {
  return (xml.match(/<si\b[^>]*>[\s\S]*?<\/si>|<si\/>/g) ?? []).map((si) =>
    decodeXmlEntities((si.match(/<t\b[^>]*>([\s\S]*?)<\/t>/g) ?? []).map((t) => t.replace(/<[^>]+>/g, "")).join("")),
  );
}

function xlsxToText(entries: Map<string, Buffer>): string {
  const shared = parseSharedStrings(entries.get("xl/sharedStrings.xml")?.toString("utf8") ?? "");
  const sheets = [...entries.keys()]
    .filter((k) => /^xl\/worksheets\/sheet\d+\.xml$/.test(k))
    .sort((a, b) => (parseInt(a.match(/(\d+)/)![0]!, 10) - parseInt(b.match(/(\d+)/)![0]!, 10)));
  if (!sheets.length) throw new Error("没有工作表");
  const parts: string[] = [];
  sheets.forEach((key, i) => {
    const xml = entries.get(key)!.toString("utf8");
    const rows = xml.match(/<row\b[^>]*>[\s\S]*?<\/row>|<row\/>/g) ?? [];
    const lines: string[] = [];
    for (const row of rows.slice(0, 5000)) {
      const cells = (row.match(/<c\b[^>]*\/>|<c\b[^>]*>[\s\S]*?<\/c>/g) ?? []).map((cell) => {
        const type = cell.match(/\bt="(\w+)"/)?.[1];
        const v = cell.match(/<v>([\s\S]*?)<\/v>/)?.[1];
        if (type === "s" && v !== undefined) return shared[Number(v)] ?? "";
        if (type === "inlineStr") {
          return decodeXmlEntities((cell.match(/<t\b[^>]*>([\s\S]*?)<\/t>/g) ?? []).map((t) => t.replace(/<[^>]+>/g, "")).join(""));
        }
        return decodeXmlEntities(v ?? "");
      });
      if (cells.some((c) => c !== "")) lines.push(cells.join("\t").replace(/\s+$/g, ""));
    }
    if (sheets.length > 1) parts.push(`--- 工作表 ${i + 1} ---`);
    parts.push(lines.join("\n").trim());
  });
  const text = parts.filter(Boolean).join("\n\n").trim();
  if (!text) throw new Error("工作表内容为空");
  return text;
}

function pptxToText(entries: Map<string, Buffer>): string {
  const slides = [...entries.keys()]
    .filter((k) => /^ppt\/slides\/slide\d+\.xml$/.test(k))
    .sort((a, b) => (parseInt(a.match(/(\d+)/)![0]!, 10) - parseInt(b.match(/(\d+)/)![0]!, 10)));
  if (!slides.length) throw new Error("没有幻灯片");
  const parts = slides.map((key, i) => {
    const text = xmlToText(entries.get(key)!.toString("utf8"), [/<a:br\b[^>]*\/>/g, /<\/a:p>/g]);
    return slides.length > 1 ? `--- 幻灯片 ${i + 1} ---\n${text}` : text;
  });
  const text = parts.filter((p) => p.replace(/---[^-]+---/, "").trim()).join("\n\n").trim();
  if (!text) throw new Error("幻灯片内容为空");
  return text;
}

// ---------------------------------------------------------------- 图片规整

/** provider 普遍接受的图片格式；其余（HEIC/AVIF/TIFF/BMP…）先转 PNG */
const NATIVE_IMAGE_MIMES = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);
/** 超过该字节数的大图先缩放（4:3 参考：约 2000 万像素 base64），控制 token 消耗 */
const RESIZE_OVER_BYTES = 2 * 1024 * 1024;
const RESIZE_MAX_EDGE = 2048;

/**
 * 把附件图片规整成 provider 友好的形态：
 * 1. 非 PNG/JPEG/WebP/GIF（HEIC/AVIF/TIFF 等）→ convertToPng；转不动再原样发（交由 provider 报错）。
 * 2. 大图（>2MB）→ resizeImage 压到 2048px 内，返回的尺寸注记由调用方拼进 prompt 文本。
 */
export async function normalizeImage(data: string, mime: string): Promise<{ data: string; mimeType: string; note?: string }> {
  let img = { data, mimeType: mime || "image/png" };
  if (!NATIVE_IMAGE_MIMES.has(img.mimeType)) {
    const converted = await convertToPng(img.data, img.mimeType);
    if (converted) img = converted;
  }
  const bytes = Math.floor((img.data.length * 3) / 4);
  if (bytes > RESIZE_OVER_BYTES) {
    try {
      const resized = await resizeImage(Buffer.from(img.data, "base64"), img.mimeType, {
        maxWidth: RESIZE_MAX_EDGE,
        maxHeight: RESIZE_MAX_EDGE,
      });
      if (resized) {
        const note = formatDimensionNote(resized);
        return {
          data: resized.data,
          mimeType: resized.mimeType,
          ...(note ? { note } : {}),
        };
      }
    } catch {
      // 缩放失败用原图（转换后的 PNG）兜底
    }
  }
  return img;
}

// ---------------------------------------------------------------- 主入口

async function extractOne(att: IncomingAttachment): Promise<{ kind: "text" | "image"; body?: string; image?: { type: "image"; data: string; mimeType: string } }> {
  const { name, data } = att;
  const size = att.size ?? Math.floor((data.length * 3) / 4);
  const ext = extOf(name);
  const mime = att.mime_type ?? "";

  if (size > MAX_INPUT_BYTES) throw new Error(`超过大小上限 ${fmtSize(MAX_INPUT_BYTES)}`);

  // 图片：直接路由到 prompt images（与「添加图片」同一通道）
  if (mime.startsWith("image/") || IMAGE_EXTS.has(ext)) {
    return { kind: "image", image: { type: "image", data, mimeType: mime || `image/${ext || "png"}` } };
  }

  // pdf / office 文档：二进制解析
  if (ext === "pdf" || mime === "application/pdf") {
    return { kind: "text", body: await pdfToText(Buffer.from(data, "base64")) };
  }
  if (["docx", "xlsx", "pptx"].includes(ext)) {
    const buf = Buffer.from(data, "base64");
    const entries = readZipEntries(buf);
    const body = ext === "docx" ? docxToText(entries) : ext === "xlsx" ? xlsxToText(entries) : pptxToText(entries);
    return { kind: "text", body };
  }
  if (["doc", "xls", "ppt"].includes(ext)) {
    throw new Error("旧版 Office 二进制格式暂不支持，请另存为 .docx / .xlsx / .pptx");
  }

  // 其余按文本处理；带 NUL 字节的拒绝为二进制
  if (!TEXT_EXTS.has(ext) && !mime.startsWith("text/") && mime !== "application/json") {
    throw new Error(`不支持的类型 .${ext || "无扩展名"}`);
  }
  const buf = Buffer.from(data, "base64");
  if (buf.subarray(0, 8192).includes(0)) throw new Error("无法识别的二进制格式");
  return { kind: "text", body: buf.toString("utf8") };
}

/** 解析全部附件；单个失败不中断，生成错误说明块。永不 reject。 */
export async function parseAttachments(list: IncomingAttachment[] | undefined): Promise<ParsedAttachments> {
  const images: ParsedAttachments["images"] = [];
  const blocks: string[] = [];
  const notes: string[] = [];
  let totalChars = 0;
  for (const att of (list ?? []).slice(0, MAX_FILES)) {
    if (!att?.data) continue;
    const size = att.size ?? Math.floor((att.data.length * 3) / 4);
    try {
      const r = await extractOne(att);
      if (r.kind === "image" && r.image) {
        const norm = await normalizeImage(r.image.data, r.image.mimeType);
        images.push({ type: "image", data: norm.data, mimeType: norm.mimeType });
        if (norm.note) notes.push(norm.note);
        continue;
      }
      if (totalChars >= MAX_TOTAL_CHARS) {
        blocks.push(errorBlock(att.name, size, "附件内容总量超出上限，未注入本文件"));
        continue;
      }
      const body = capChars(r.body ?? "");
      totalChars += body.length;
      blocks.push(block(att.name, size, body));
    } catch (err) {
      blocks.push(errorBlock(att.name, size, String(err instanceof Error ? err.message : err)));
    }
  }
  return { images, blocks, notes };
}
