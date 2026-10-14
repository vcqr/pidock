<script lang="ts">
/**
 * MCP 服务器表单的可复用字段集合：服务器详情页「通用」标签（新建/编辑共用）。
 * 字段状态由父组件持有（v-model），保存/探测逻辑也留在父组件。
 * 布局：整页单列全宽，低频字段（环境变量/工作目录/超时）收进「高级设置」。
 */
export interface McpFormState {
  name: string;
  type: "stdio" | "http" | "streamable-http";
  command: string;
  /** 多行，每行一个参数 */
  args: string;
  url: string;
  /** 多行 KEY=value（stdio 环境变量） */
  env: string;
  /** 多行 KEY=value（http 请求头，认证 token 写在这里） */
  headers: string;
  description: string;
  /** 超时（秒），空串 = pi 默认 60s */
  timeout: string;
  cwd: string;
  advancedOpen: boolean;
}
</script>

<script setup lang="ts">
import Icon from "./Icon.vue";
import SelectMenu, { type SelectOption } from "./SelectMenu.vue";

const form = defineModel<McpFormState>({ required: true });

// pi 官方约定（pi.dev/docs/mcp）：type 取 stdio | http | streamable-http，
// SSE 不受支持；http 即 streamable HTTP，请求头用于认证
const typeOptions: SelectOption[] = [
  { value: "stdio", label: "标准输入 / 输出", hint: "stdio" },
  { value: "http", label: "HTTP", hint: "http" },
  { value: "streamable-http", label: "可流式传输的 HTTP", hint: "streamable-http" },
];
</script>

<template>
  <div>
    <div class="field">
      <label>名称 <i>*</i></label>
      <input v-model="form.name" placeholder="名称" />
    </div>

    <div class="field">
      <label>类型 <i>*</i></label>
      <SelectMenu v-model="form.type" :options="typeOptions" />
    </div>

    <div class="field">
      <label>描述</label>
      <input v-model="form.description" placeholder="这个服务器提供什么能力（写入 mcp.json，帮助模型挑选工具）" />
    </div>

    <template v-if="form.type === 'stdio'">
      <div class="field">
        <label>命令 <i>*</i></label>
        <input v-model="form.command" placeholder="obscura mcp 或 uvx -y pkg" />
        <p class="hint">可粘贴完整命令行（支持引号）：首个可执行文件之外的 token 自动拆为参数；也可在「参数」中逐行填写。</p>
      </div>
      <div class="field">
        <label>参数</label>
        <textarea v-model="form.args" rows="3" placeholder="arg1&#10;arg2" spellcheck="false"></textarea>
      </div>
      <div class="advanced">
        <button type="button" class="adv-head" @click="form.advancedOpen = !form.advancedOpen">
          高级设置
          <span class="adv-chev" :class="{ open: form.advancedOpen }"><Icon name="arrow-down-s-line" :size="13" /></span>
        </button>
        <template v-if="form.advancedOpen">
          <div class="field">
            <label>环境变量</label>
            <textarea v-model="form.env" rows="3" placeholder="KEY=value&#10;TOKEN=${MY_TOKEN}" spellcheck="false"></textarea>
          </div>
          <div class="field">
            <label>工作目录</label>
            <input v-model="form.cwd" placeholder="留空使用会话工作目录" />
          </div>
          <div class="field">
            <label>超时（秒）</label>
            <input v-model="form.timeout" inputmode="numeric" placeholder="60" />
          </div>
        </template>
      </div>
    </template>

    <template v-else>
      <div class="field">
        <label>URL <i>*</i></label>
        <input v-model="form.url" placeholder="https://mcp.example.com/mcp" />
      </div>
      <div class="field">
        <label>请求头</label>
        <textarea
          v-model="form.headers"
          rows="3"
          placeholder="Authorization=Bearer your-token&#10;X-Api-Key=${MY_KEY}"
          spellcheck="false"
        ></textarea>
        <p class="hint">每行一个 KEY=value，认证 token 写在这里；值支持 ${VAR} 引用环境变量或 !cmd 取命令输出。</p>
      </div>
      <div class="field">
        <label>超时（秒）</label>
        <input v-model="form.timeout" inputmode="numeric" placeholder="60" />
      </div>
    </template>
  </div>
</template>

<style scoped>
.field {
  margin-top: 16px;
  min-width: 0;
}
.field label {
  display: block;
  font-size: calc(13px * var(--pd-font-scale));
  color: var(--pd-text-2);
  margin-bottom: 7px;
}
.field label i {
  color: var(--pd-red);
  font-style: normal;
}
.field input,
.field textarea {
  width: 100%;
  box-sizing: border-box;
  background: var(--pd-bg-card);
  border: 1px solid var(--pd-border);
  border-radius: 9px;
  color: var(--pd-text);
  font-size: calc(13px * var(--pd-font-scale));
  font-family: inherit;
  padding: 9px 12px;
}
.field textarea {
  resize: vertical;
  line-height: 1.6;
}
.field input:focus,
.field textarea:focus {
  outline: none;
  border-color: var(--pd-accent);
}
.field input::placeholder,
.field textarea::placeholder {
  color: var(--pd-text-4);
}
.field textarea {
  font-family: var(--pd-mono);
  font-size: calc(12.5px * var(--pd-font-scale));
}

.advanced {
  margin-top: 16px;
  border-top: 1px solid var(--pd-border-soft);
  padding-top: 12px;
}
.adv-head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  background: none;
  border: none;
  color: var(--pd-text-2);
  font-size: calc(13px * var(--pd-font-scale));
  font-weight: 600;
  cursor: pointer;
  padding: 2px 0;
}
.adv-chev {
  display: grid;
  place-items: center;
  color: var(--pd-text-4);
  transition: transform 0.12s;
}
.adv-chev.open {
  transform: rotate(180deg);
}
.advanced .field {
  margin-top: 12px;
}

.hint {
  margin: 7px 0 0;
  font-size: calc(12px * var(--pd-font-scale));
  color: var(--pd-text-4);
  line-height: 1.6;
}
</style>
