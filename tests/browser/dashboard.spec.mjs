import { expect, test } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const binary = process.env.OPENFRAG_TEST_PRODUCTION_BINARY
  ?? join(process.env.OPENFRAG_TEST_ARTIFACTS ?? '', 'openfragd-production');
const port = process.env.OPENFRAG_TEST_PORT;
const baseURL = `http://127.0.0.1:${port}`;
const root = join(tmpdir(), 'browser-dashboard');
const dataDirectory = join(root, 'data');
let daemon;
let daemonOutput = '';

async function waitForHealth() {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (daemon.exitCode !== null) {
      throw new Error(`daemon exited before browser test readiness\n${daemonOutput}`);
    }
    try {
      const response = await fetch(`${baseURL}/api/health`);
      if (response.ok) return;
    } catch {
      // The loopback listener may not be ready yet.
    }
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error(`daemon did not become ready\n${daemonOutput}`);
}

test.beforeAll(async () => {
  if (!binary || !port) throw new Error('suite did not provide the production binary and deterministic port');
  rmSync(root, { recursive: true, force: true });
  mkdirSync(dataDirectory, { recursive: true, mode: 0o700 });
  daemon = spawn(binary, ['serve', '--bind', `127.0.0.1:${port}`, '--data-dir', dataDirectory], {
    env: process.env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  daemon.stdout.on('data', chunk => { daemonOutput += chunk.toString(); });
  daemon.stderr.on('data', chunk => { daemonOutput += chunk.toString(); });
  await waitForHealth();
});

test.afterAll(async () => {
  if (daemon && daemon.exitCode === null) {
    daemon.kill('SIGTERM');
    await new Promise(resolve => daemon.once('exit', resolve));
  }
  rmSync(root, { recursive: true, force: true });
});

test('real browser completes the local dashboard navigation and honest failure journey', async ({ page }) => {
  const remoteRequests = [];
  const consoleErrors = [];
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== baseURL) {
      remoteRequests.push(url.href);
      await route.abort('blockedbyclient');
      return;
    }
    await route.continue();
  });
  page.on('console', message => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });

  await page.goto(baseURL);
  await expect(page.getByRole('heading', { name: 'Tonight', exact: true })).toBeVisible();
  await expect(page.locator('#rating-state')).toContainText('Rating unavailable');
  await expect(page.locator('#recent-matches')).toContainText('No imported matches yet');

  await page.getByRole('button', { name: 'Matches' }).click();
  await expect(page.getByRole('heading', { name: 'Matches', exact: true })).toBeVisible();
  await expect(page.locator('#match-list')).toContainText('No imported matches yet');

  await page.getByRole('button', { name: 'Clips' }).click();
  await expect(page.getByRole('heading', { name: 'Clip review' })).toBeVisible();
  await expect(page.locator('#clip-list')).toContainText('No clips are ready for review');

  await page.getByRole('button', { name: 'Diagnostics' }).click();
  await expect(page.getByRole('heading', { name: 'Capture and GSI diagnostics' })).toBeVisible();
  await expect(page.locator('#diagnostics-output')).toContainText('capture');

  await page.getByRole('button', { name: 'Setup' }).click();
  await expect(page.getByRole('heading', { name: 'First-run setup' })).toBeVisible();
  await expect(page.locator('#setup-output')).toContainText('storage');
  await expect(page.locator('#setup-completion')).not.toBeEmpty();

  await page.getByRole('button', { name: 'Tonight' }).click();
  await page.getByRole('button', { name: 'Manual Flag' }).click();
  await expect(page.locator('#announcements')).toContainText('Manual Flag unavailable');
  await expect(page.getByRole('button', { name: 'Tonight' })).toHaveAttribute('aria-current', 'true');

  expect(remoteRequests).toEqual([]);
  expect(consoleErrors).toEqual([
    'Failed to load resource: the server responded with a status of 503 (Service Unavailable)',
  ]);
});
