// Harmless native certification target. Not bundled with Deck.
#include <cstdio>
#include <cstring>
extern "C" void *deck_bergamot_load(const char *) noexcept;
extern "C" int deck_bergamot_translate(void *, const char *, size_t, char **, size_t *) noexcept;
extern "C" void deck_bergamot_free(char *) noexcept;
extern "C" void deck_bergamot_unload(void *) noexcept;

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  auto *model = deck_bergamot_load(argv[1]);
  if (!model) return 3;
  const char *source = "The build completed successfully.";
  char *output = nullptr;
  size_t size = 0;
  const int ok = deck_bergamot_translate(model, source, std::strlen(source), &output, &size);
  if (ok && output && size) std::fwrite(output, 1, size, stdout);
  deck_bergamot_free(output);
  deck_bergamot_unload(model);
  return ok && size ? 0 : 4;
}
