import { createHash } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import { join } from "node:path";
import {
  ATTACHMENT_PREVIEW_BYTES,
  ATTACHMENT_THRESHOLD_BYTES,
  INLINE_PREVIEW_BYTES,
  type AttachmentRef,
  type Block,
  type CompactionSummaryPayload,
  type MessageCompletePayload,
  type SessionMetaPayload,
  type UsageInfo,
} from "@pidock/protocol";

/*
 * Mapping helpers shared by the live pipeline (pi events) and replay
 * (persisted JSONL entries -> persistable envelopes).
 */

type Unknown = Record<string, any>;

/** attachment-ize any value that exceeds the inline threshold */
export function maybeAttach(text: string): { text: string; attachment?: AttachmentRef } {
  if (text.length <= ATTACHMENT_THRESHOLD_BYTES) return { text };
  const sha256 = createHash("sha256").update(text, "utf8").digest("hex");
  // content-addressed local copy; the sync layer uploads it to object storage
  try {
    const dir = join(getAgentDir(), "pidock", "attachments");
    mkdirSync(dir, { recursive: true });
    const file = join(dir, sha256);
    if (!existsSync(file)) writeFileSync(file, text, "utf8");
  } catch {
    // best effort; sync will skip upload when the file is missing
  }
  return {
    text: text.slice(0, ATTACHMENT_PREVIEW_BYTES),
    attachment: {
      // v1: content-addressed id; actual object upload happens in the sync layer (M4)
      attachment_id: sha256,
      sha256,
      size: Buffer.byteLength(text, "utf8"),
      preview: text.slice(0, ATTACHMENT_PREVIEW_BYTES),
      truncated: true,
    },
  };
}

export function previewOf(text: string, limit = INLINE_PREVIEW_BYTES): string {
  return text.length > limit ? text.slice(0, limit) : text;
}

/** AgentMessage -> blocks + metadata for a persisted message_complete event */
export function messageToPayload(msg: Unknown, messageId?: string): MessageCompletePayload {
  const role = msg.role as MessageCompletePayload["role"];
  const blocks: Block[] = [];

  if (role === "user") {
    const content = msg.content;
    if (typeof content === "string") {
      blocks.push({ type: "text", text: content });
    } else if (Array.isArray(content)) {
      for (const part of content) {
        if (part?.type === "text") blocks.push({ type: "text", text: part.text });
        else if (part?.type === "image") {
          // 图片随条目持久化（base64 内联），供 UI 历史回放显示
          blocks.push({ type: "image", data: String(part.data ?? ""), mime: String(part.mimeType ?? "image/png") });
        }
      }
    }
  } else if (role === "assistant") {
    for (const part of (msg.content ?? []) as Unknown[]) {
      if (part?.type === "text") {
        blocks.push({ type: "text", text: part.text });
      } else if (part?.type === "thinking") {
        blocks.push({ type: "thinking", thinking: part.thinking ?? "" });
      } else if (part?.type === "toolCall") {
        const argsJson = JSON.stringify(part.arguments ?? {});
        const { text, attachment } = maybeAttach(argsJson);
        blocks.push({
          type: "toolCall",
          callId: part.id,
          toolName: part.name,
          args: text,
          ...(attachment ? { attachment } : {}),
        });
      }
    }
  } else if (role === "toolResult") {
    const texts: string[] = [];
    for (const part of (msg.content ?? []) as Unknown[]) {
      if (part?.type === "text") texts.push(part.text);
    }
    const { text, attachment } = maybeAttach(texts.join("\n"));
    blocks.push({
      type: "toolResult",
      callId: msg.toolCallId,
      toolName: msg.toolName,
      isError: Boolean(msg.isError),
      output: text,
      ...(attachment ? { attachment } : {}),
    });
  }

  const payload: MessageCompletePayload = { role, blocks, entry_id: "" };
  if (messageId !== undefined) payload.message_id = messageId;
  if (role === "assistant") {
    payload.provider = msg.provider;
    payload.model = msg.model;
    payload.stopReason = msg.stopReason;
    if (msg.errorMessage) payload.errorMessage = msg.errorMessage;
    if (msg.usage) {
      const u = msg.usage;
      const usage: UsageInfo = {
        input: u.input ?? 0,
        output: u.output ?? 0,
        cacheRead: u.cacheRead ?? 0,
        cacheWrite: u.cacheWrite ?? 0,
        totalTokens: u.totalTokens ?? 0,
        costTotal: u.cost?.total ?? 0,
      };
      payload.usage = usage;
    }
  }
  return payload;
}

export function entryToPayload(
  entry: Unknown,
  messageId?: string,
): { kind: string; payload: unknown } | null {
  switch (entry?.type) {
    case "message": {
      const mapped = messageToPayload(entry.message, messageId);
      mapped.entry_id = typeof entry.id === "string" ? entry.id : "";
      // 助手消息生成耗时：消息开始（msg.timestamp，ms epoch）到条目落盘（entry.timestamp）
      if (entry.message?.role === "assistant") {
        const start = typeof entry.message.timestamp === "number" ? entry.message.timestamp : NaN;
        const end = typeof entry.timestamp === "string" ? Date.parse(entry.timestamp) : NaN;
        if (Number.isFinite(start) && Number.isFinite(end) && end >= start) {
          mapped.duration_ms = end - start;
        }
      }
      return { kind: "message_complete", payload: mapped };
    }
    case "model_change":
    case "session_info": {
      const meta: SessionMetaPayload = { entry_id: typeof entry.id === "string" ? entry.id : "" };
      if (entry.type === "model_change") {
        meta.provider = entry.provider;
        meta.model = entry.modelId;
      } else {
        if (entry.name !== undefined) meta.name = entry.name;
      }
      return { kind: "session_meta", payload: meta };
    }
    case "compaction": {
      const payload: CompactionSummaryPayload = {
        summary: typeof entry.summary === "string" ? entry.summary : "",
        entry_id: typeof entry.id === "string" ? entry.id : "",
        ...(typeof entry.tokensBefore === "number" ? { tokens_before: entry.tokensBefore } : {}),
      };
      return { kind: "compaction_summary", payload };
    }
    // branch_summary / custom / custom_message / label / thinking_level_change:
    // not surfaced in v1
    default:
      return null;
  }
}

export function metaPayloadFromSession(s: {
  cwd: string;
  provider?: string;
  model?: string;
  expert_id?: string;
  expert_name?: string;
  parent_session_id?: string;
}): SessionMetaPayload {
  const meta: SessionMetaPayload = { cwd: s.cwd };
  if (s.provider) meta.provider = s.provider;
  if (s.model) meta.model = s.model;
  if (s.expert_id) meta.expert_id = s.expert_id;
  if (s.expert_name) meta.expert_name = s.expert_name;
  if (s.parent_session_id) meta.parent_session_id = s.parent_session_id;
  return meta;
}
