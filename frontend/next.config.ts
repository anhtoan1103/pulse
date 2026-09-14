import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Smaller, self-contained production image for the Docker build below.
  output: "standalone",
  // `next dev`'s HMR websocket rejects cross-origin requests by default;
  // needed when the dev server is reached via 127.0.0.1 or a LAN IP instead
  // of localhost (e.g. driving it from a headless browser). No effect on
  // `next build`/`next start` — dev-only.
  allowedDevOrigins: ["127.0.0.1", "localhost"],
};

export default nextConfig;
