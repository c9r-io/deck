// The explicit legacy-only Keychain action. Confirmation and refresh stay in
// Settings; this small flow keeps cancel, partial failure and retry testable.
export async function removeLegacySlackCredentials({ confirm, invoke, refresh, notice }) {
  if (!(await confirm())) return;
  try {
    await invoke('slack_legacy_credentials_clear');
    notice('cleared');
  } catch (_) {
    notice('error');
  }
  await refresh();
}
