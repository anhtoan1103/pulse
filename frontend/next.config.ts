import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Smaller, self-contained production image for the Docker build below.
  output: "standalone",
};

export default nextConfig;
