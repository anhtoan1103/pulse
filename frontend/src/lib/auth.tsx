"use client";

import { createContext, useContext, useEffect, useState, useCallback, ReactNode } from "react";
import { useRouter } from "next/navigation";
import * as api from "./api";

type Status = "loading" | "authenticated" | "unauthenticated";

interface AuthContextValue {
  status: Status;
  user: api.CurrentUser | null;
  login: (email: string, password: string) => Promise<void>;
  register: (email: string, password: string) => Promise<void>;
  logout: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status>("loading");
  const [user, setUser] = useState<api.CurrentUser | null>(null);

  useEffect(() => {
    const token = api.getToken();
    if (!token) {
      // Synchronizing with an external system (localStorage token
      // presence), not deriving from props/state — the standard "fetch in
      // an effect" shape from https://react.dev/learn/you-might-not-need-an-effect.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setStatus("unauthenticated");
      return;
    }
    api
      .me()
      .then((u) => {
        setUser(u);
        setStatus("authenticated");
      })
      .catch(() => {
        api.setToken(null);
        setStatus("unauthenticated");
      });
  }, []);

  // Any request elsewhere in the app (a background poll, a mutation) that
  // gets a 401 calls this — a token expiring or an admin disabling the
  // account mid-session must flip `status` here, not just clear storage,
  // or RequireAuth never learns to redirect and the user is stuck staring
  // at a stale "authenticated" shell repeating the same failed request.
  useEffect(() => {
    return api.onUnauthorized(() => {
      setUser(null);
      setStatus("unauthenticated");
    });
  }, []);

  const login = useCallback(async (email: string, password: string) => {
    const { token, user } = await api.login(email, password);
    api.setToken(token);
    // /auth/login's response is a compact user summary; /auth/me has the
    // full CurrentUser shape (created_at) — fetch it so both paths agree.
    const full = await api.me().catch(() => ({ ...user, created_at: "" }) as api.CurrentUser);
    setUser(full);
    setStatus("authenticated");
  }, []);

  const register = useCallback(
    async (email: string, password: string) => {
      await api.register(email, password);
      await login(email, password);
    },
    [login],
  );

  const logout = useCallback(() => {
    api.logout().catch(() => {}); // best-effort; token is discarded regardless
    api.setToken(null);
    setUser(null);
    setStatus("unauthenticated");
  }, []);

  return (
    <AuthContext.Provider value={{ status, user, login, register, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}

/** Wraps a page's content; redirects to /login until a session is confirmed. */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { status } = useAuth();
  const router = useRouter();

  useEffect(() => {
    if (status === "unauthenticated") router.replace("/login");
  }, [status, router]);

  if (status !== "authenticated") {
    return (
      <div className="flex flex-1 items-center justify-center py-24 text-sm text-zinc-500 dark:text-zinc-400">
        Loading…
      </div>
    );
  }
  return <>{children}</>;
}
