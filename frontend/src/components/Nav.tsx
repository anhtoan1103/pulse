"use client";

import Link from "next/link";
import { useAuth } from "@/lib/auth";

export function Nav() {
  const { status, user, logout } = useAuth();

  return (
    <header className="border-b border-zinc-200 dark:border-zinc-800">
      <div className="mx-auto flex w-full max-w-5xl items-center justify-between px-4 py-3 sm:px-6">
        <Link href="/" className="flex items-center gap-2 font-semibold tracking-tight">
          <span className="inline-block h-2 w-2 rounded-full bg-emerald-500" aria-hidden />
          Pulse
        </Link>

        {status === "authenticated" && (
          <nav className="flex items-center gap-4 text-sm">
            <Link href="/" className="text-zinc-600 hover:text-zinc-950 dark:text-zinc-400 dark:hover:text-zinc-50">
              Endpoints
            </Link>
            <Link
              href="/incidents"
              className="text-zinc-600 hover:text-zinc-950 dark:text-zinc-400 dark:hover:text-zinc-50"
            >
              Incidents
            </Link>
            {user?.role === "admin" && (
              <Link
                href="/admin"
                className="text-zinc-600 hover:text-zinc-950 dark:text-zinc-400 dark:hover:text-zinc-50"
              >
                Admin
              </Link>
            )}
            <span className="hidden text-zinc-400 sm:inline dark:text-zinc-600">{user?.email}</span>
            <button
              onClick={logout}
              className="rounded-md border border-zinc-300 px-2.5 py-1 text-zinc-700 hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-300 dark:hover:bg-zinc-900"
            >
              Log out
            </button>
          </nav>
        )}
      </div>
    </header>
  );
}
