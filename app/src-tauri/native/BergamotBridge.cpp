// In-process C ABI around the pinned Bergamot/Marian static libraries.
// Source and output text are confined to caller-owned memory; no logging.
#include "translator/parser.h"
#include "translator/response.h"
#include "translator/response_options.h"
#include "translator/service.h"
#include <cstdlib>
#include <cstring>
#include <future>
#include <memory>
#include <string>

using namespace marian::bergamot;

struct DeckBergamot {
  std::unique_ptr<AsyncService> service;
  std::shared_ptr<TranslationModel> model;
};

extern "C" void *deck_bergamot_load(const char *directory) noexcept {
  if (!directory) return nullptr;
  try {
    const std::string root(directory);
    if (root.empty()) return nullptr;
    // YAML::Node quotes paths, including spaces and Unicode, without writing
    // a configuration file containing private data.
    YAML::Node config;
    config["bergamot-mode"] = "wasm";
    config["models"].push_back(root + "/model.enzh.intgemm.alphas.bin");
    config["vocabs"].push_back(root + "/srcvocab.enzh.spm");
    config["vocabs"].push_back(root + "/trgvocab.enzh.spm");
    config["shortlist"].push_back(root + "/lex.50.50.enzh.s2t.bin");
    config["shortlist"].push_back(false);
    config["beam-size"] = 1;
    config["normalize"] = 1.0;
    config["word-penalty"] = 0;
    config["max-length-break"] = 128;
    config["mini-batch-words"] = 1024;
    config["workspace"] = 128;
    config["max-length-factor"] = 2.0;
    config["skip-cost"] = true;
    config["cpu-threads"] = 2;
    config["quiet"] = true;
    config["quiet-translation"] = true;
    config["gemm-precision"] = "int8shiftAlphaAll";
    config["alignment"] = "soft";
    config["ssplit-mode"] = "sentence";
    auto handle = std::make_unique<DeckBergamot>();
    AsyncService::Config serviceConfig;
    serviceConfig.numWorkers = 2;
    handle->service = std::make_unique<AsyncService>(serviceConfig);
    handle->model = handle->service->createCompatibleModel(parseOptionsFromString(YAML::Dump(config)));
    return handle.release();
  } catch (...) {
    return nullptr;
  }
}

extern "C" int deck_bergamot_translate(void *opaque, const char *input, size_t input_len,
                                         char **output, size_t *output_len) noexcept {
  if (!opaque || !input || !output || !output_len) return 0;
  *output = nullptr;
  *output_len = 0;
  try {
    auto *handle = static_cast<DeckBergamot *>(opaque);
    std::promise<Response> promise;
    auto future = promise.get_future();
    auto callback = [&promise](Response &&response) { promise.set_value(std::move(response)); };
    ResponseOptions options;
    options.HTML = true;
    handle->service->translate(handle->model, std::string(input, input_len), callback, options);
    auto response = future.get();
    const auto &text = response.target.text;
    char *copy = static_cast<char *>(std::malloc(text.size() + 1));
    if (!copy) return 0;
    std::memcpy(copy, text.data(), text.size());
    copy[text.size()] = '\0';
    *output = copy;
    *output_len = text.size();
    return 1;
  } catch (...) {
    return 0;
  }
}

extern "C" void deck_bergamot_free(char *output) noexcept { std::free(output); }
extern "C" void deck_bergamot_unload(void *opaque) noexcept {
  delete static_cast<DeckBergamot *>(opaque);
}
