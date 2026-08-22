import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

const packageRoot = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  resolve: {
    alias: {
      '@': path.join(packageRoot, 'src'),
      shared: path.join(packageRoot, '../../shared'),
    },
  },
  test: {
    environment: 'node',
  },
});
