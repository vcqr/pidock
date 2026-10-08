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

/** /auth/methods 响应：登录页据此渲染可用登录方式（拉取失败按仅密码处理） */
export interface AuthMethods {
  password: boolean;
  ldap: { enabled: boolean };
  oidc: { enabled: boolean; label?: string };
}

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

  async request(path: string, init?: RequestInit): Promise<any> {
    const url = `${this.state.server_url}${path}`;
    let res = await fetch(url, {
      ...init,
      headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
    });
    if (res.status === 401) {
      if (await this.refreshTokens()) {
        res = await fetch(url, {
          ...init,
          headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
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
  ): Promise<void> {
    const res = await fetch(`${serverUrl}/auth/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, password, invite_code: inviteCode.trim() }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `注册失败 (${res.status})`);
    }
  }

  async login(serverUrl: string, email: string, password: string): Promise<AuthState> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    return this.adoptLogin(serverUrl, email, res, "登录失败");
  }

  /** LDAP 登录：独立端点，用户名走 LDAP 目录（uid/mail 由服务端 user_filter 决定） */
  async loginLdap(serverUrl: string, username: string, password: string): Promise<AuthState> {
    const res = await fetch(`${serverUrl.replace(/\/+$/, "")}/auth/login/ldap`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ username, password }),
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
    const state: AuthState = {
      server_url: serverUrl,
      access_token: data.access_token,
      refresh_token: data.refresh_token,
      user_id: data.user_id,
      email: data.email ?? account,
      role: data.role,
    };
    this.state = state;
    saveAuth(state);
    return state;
  }
}
