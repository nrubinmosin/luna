### Removed

- Removed the option to copy a source account's login with
  `test-accounts.ps1 clone`. Throwaway accounts get their own sign-in so a
  token refresh in a clone cannot sign the source account out.
