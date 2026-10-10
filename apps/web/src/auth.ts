/**
 * Web auth: token pair in localStorage + auto-refresh on 401.
 */
export interface AuthState {
  server_url: string;
  access_token: string;
  refresh_token: string;
  user_id: string;
  email: string;
  /** "admin" | "user"，登录/注册时服务端下发；仅用于 UI 显隐 */
  role?: string;
}

const KEY = "pidock.web.auth";

/** AuthClient 的结构化子集：组件 props 用它而不是类本身，
 *  vue-tsc 对模板 ref 解包出的类型会丢掉类的私有字段，类类型会误报不兼容 */
export interface ApiClient {
  readonly token: string;
  readonly userId: string;
  request(path: string, init?: RequestInit): Promise<any>;
}

export function loadAuth(): AuthState | null {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as AuthState) : null;
  } catch {
    return null;
  }
}

export function saveAuth(state: AuthState): void {
  localStorage.setItem(KEY, JSON.stringify(state));
}

export function clearAuth(): void {
  localStorage.removeItem(KEY);
}

/**
 * 邮箱格式校验，与服务端 apps/server/src/auth.rs 的 valid_email 同规则：
 * 无空白、恰好一个 @、local 非空、域名非空且不含 @、必须含点、
 * 不以点开头/结尾、无连续点。（`[^\s@.]+` 分段匹配同时排除连续点）
 */
export function validEmail(email: string): boolean {
  return /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/.test(email.trim());
}

/** /auth/methods 响应：登录页据此渲染可用登录方式（拉取失败按仅密码处理） */
export interface AuthMethods {
  password: boolean;
  /** 注册开关（管理员可全局关闭）；缺省视为允许 */
  register?: { allowed: boolean };
  ldap: { enabled: boolean };
  oidc: { enabled: boolean; label?: string };
  /** 启用时登录/注册前须过 Turnstile 人机验证，请求附 turnstile_token；site_key 渲染 widget 用 */
  turnstile?: { enabled: boolean; site_key?: string };
  /** 启用时密码登录走两步验证：先发邮箱验证码，再凭码换 token */
  login_code?: { enabled: boolean };
}

/** 登录两步验证中间态：密码已通过，等待邮箱验证码（此时不发 token） */
export interface LoginPending {
  pending: "email_code";
  challenge: string;
  /** 脱敏邮箱，如 u***@example.com */
  email: string;
}

/** 注册两段式中间态：预检已过、验证码已发，验证通过才建号并自动登录 */
export interface RegisterPending {
  pending: "register_code";
  challenge: string;
  email: string;
}

/** register() 的两种结局：直接建号成功（未开注册验证码）或进入验证码步骤 */
export type RegisterResult = { user_id: string; role: string } | RegisterPending;

export async function fetchAuthMethods(serverUrl: string): Promise<AuthMethods | null> {
  try {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/methods`);
    if (!res.ok) return null;
    return (await res.json()) as AuthMethods;
  } catch {
    return null;
  }
}

/** SSO 回调后用一次性 code 换正式 token 对；email 由服务端从 IdP 同步 */
export async function exchangeSsoCode(serverUrl: string, code: string): Promise<AuthState> {
  const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/sso/exchange`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res.ok) {
    const data = await res.json().catch(() => ({}));
    throw new ApiError(res.status, data?.error ?? `SSO 登录失败 (${res.status})`);
  }
  const data = await res.json();
  return {
    server_url: serverUrl,
    access_token: data.access_token,
    refresh_token: data.refresh_token,
    user_id: data.user_id,
    email: data.email ?? "",
    role: data.role,
  };
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export class AuthClient {
  constructor(
    private state: AuthState,
    private onAuthLost: () => void,
  ) {}

  get token(): string {
    return this.state.access_token;
  }

  get userId(): string {
    return this.state.user_id;
  }

  get email(): string {
    return this.state.email;
  }

  get role(): string {
    return this.state.role ?? "user";
  }

  get serverUrl(): string {
    return this.state.server_url;
  }

  update(state: AuthState): void {
    this.state = state;
    saveAuth(state);
  }

  private async refreshTokens(): Promise<boolean> {
    try {
      const res = await fetch(`${this.state.server_url}/auth/refresh`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ refresh_token: this.state.refresh_token }),
      });
      if (!res.ok) return false;
      const data = await res.json();
      this.update({ ...this.state, access_token: data.access_token, refresh_token: data.refresh_token });
      return true;
    } catch {
      return false;
    }
  }

  /** 登出：尽力而为吊销服务端 refresh token（结果不影响本地清理） */
  async logoutRemote(): Promise<void> {
    try {
      await fetch(`${this.state.server_url.replace(/\/+$/, "")}/auth/logout`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ refresh_token: this.state.refresh_token }),
      });
    } catch {
      // 尽力而为：网络失败也照常清本地
    }
  }

  async request(path: string, init?: RequestInit): Promise<any> {
    const url = `${this.state.server_url}${path}`;
    let res = await fetch(url, {
      ...init,
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${this.state.access_token}`,
        ...(init?.headers ?? {}),
      },
    });
    if (res.status === 401) {
      if (await this.refreshTokens()) {
        res = await fetch(url, {
          ...init,
          headers: {
            "content-type": "application/json",
            authorization: `Bearer ${this.state.access_token}`,
            ...(init?.headers ?? {}),
          },
        });
      }
    }
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      if (res.status === 401) this.onAuthLost();
      throw new ApiError(res.status, data?.error ?? `${res.status} ${path}`);
    }
    return data;
  }

  async register(
    serverUrl: string,
    email: string,
    password: string,
    inviteCode: string,
    turnstileToken = "",
  ): Promise<RegisterResult> {
    const res = await fetch(`${serverUrl}/auth/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        email,
        password,
        invite_code: inviteCode.trim(),
        turnstile_token: turnstileToken,
      }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `注册失败 (${res.status})`);
    }
    return (await res.json()) as RegisterResult;
  }

  /** 注册两段式第二步：验证码通过 → 服务端建号并直接发 token 对（自动登录） */
  async verifyRegisterCode(serverUrl: string, challenge: string, code: string): Promise<AuthState> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/register/code/verify`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ challenge, code: code.trim() }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `验证码校验失败 (${res.status})`);
    }
    const data = await res.json();
    return this.adoptData(serverUrl, data.email ?? "", data, "注册失败");
  }

  async login(
    serverUrl: string,
    email: string,
    password: string,
    turnstileToken = "",
  ): Promise<AuthState | LoginPending> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, password, turnstile_token: turnstileToken }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `登录失败 (${res.status})`);
    }
    const data = await res.json();
    // 两步验证：密码已通过，返回待验证中间态，由调用方进入验证码步骤
    if (data?.pending === "email_code") return data as LoginPending;
    return this.adoptData(serverUrl, email, data, "登录失败");
  }

  /** 两步验证第二步：challenge + 邮箱验证码换正式 token 对 */
  async verifyLoginCode(serverUrl: string, challenge: string, code: string): Promise<AuthState> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/login/code/verify`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ challenge, code: code.trim() }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `验证码校验失败 (${res.status})`);
    }
    const data = await res.json();
    return this.adoptData(serverUrl, data.email ?? "", data, "登录失败");
  }

  /** 重发验证码（登录/注册挑战共用，服务端 60s 冷却 + 每邮箱小时配额） */
  async resendCode(
    serverUrl: string,
    challenge: string,
  ): Promise<{ challenge: string; email: string }> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/code/resend`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ challenge }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `重发失败 (${res.status})`);
    }
    return res.json();
  }

  /** LDAP 登录：独立端点，用户名走 LDAP 目录（uid/mail 由服务端 user_filter 决定） */
  async loginLdap(
    serverUrl: string,
    username: string,
    password: string,
    turnstileToken = "",
  ): Promise<AuthState> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/login/ldap`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ username, password, turnstile_token: turnstileToken }),
    });
    return this.adoptLogin(serverUrl, username, res, "LDAP 登录失败");
  }

  private async adoptLogin(
    serverUrl: string,
    account: string,
    res: Response,
    fallbackMsg: string,
  ): Promise<AuthState> {
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `${fallbackMsg} (${res.status})`);
    }
    const data = await res.json();
    return this.adoptData(serverUrl, account, data, fallbackMsg);
  }

  /** 解析登录成功响应并落地 token 对 */
  private adoptData(
    serverUrl: string,
    account: string,
    data: Record<string, unknown>,
    fallbackMsg: string,
  ): AuthState {
    const state: AuthState = {
      server_url: serverUrl,
      access_token: data.access_token as string,
      refresh_token: data.refresh_token as string,
      user_id: data.user_id as string,
      email: (data.email as string) ?? account,
      role: data.role as string | undefined,
    };
    this.state = state;
    saveAuth(state);
    return state;
  }
}
