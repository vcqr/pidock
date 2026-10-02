/**
 * Web auth: token pair in localStorage + auto-refresh on 401.
 */
export interface AuthState {
  server_url: string;
  access_token: string;
  refresh_token: string;
  user_id: string;
  email: string;
}

const KEY = "pidock.web.auth";

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

  async register(serverUrl: string, email: string, password: string): Promise<void> {
    const res = await fetch(`${serverUrl}/auth/register`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `注册失败 (${res.status})`);
    }
  }

  async login(serverUrl: string, email: string, password: string): Promise<AuthState> {
    const res = await fetch(`${serverUrl}/auth/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, data?.error ?? `登录失败 (${res.status})`);
    }
    const data = await res.json();
    const state: AuthState = {
      server_url: serverUrl,
      access_token: data.access_token,
      refresh_token: data.refresh_token,
      user_id: data.user_id,
      email,
    };
    this.state = state;
    saveAuth(state);
    return state;
  }
}
