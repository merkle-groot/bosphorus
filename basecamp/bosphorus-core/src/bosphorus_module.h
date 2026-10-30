#pragma once

#include <atomic>
#include <cstdint>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include <logos_module_context.h>

extern "C" {
#include "bosphorus_ffi.h"
}

// Bosphorus core module (universal, Qt-free): the app API of ffi/ (docs/design/app-api.md) for a
// Basecamp view. It keeps its own wallet in the module's persistence dir (`wallet/`, jobs in
// `app/`), never the LEZ wallet module's (docs/spikes/B1-basecamp-wallet.md).
//
// The host times out calls after 20 s, and a private tx proves for ~40 s, so nothing here
// blocks: `start` runs one request on a worker thread and returns a ticket at once, `take`
// returns its answer once there is one, and `events` drains job events without waiting. The view
// polls both.
//
// Requests to `start`: `create_wallet` {"sequencer", "mnemonic"?, "sync_from"?}, `open`
// {"lending", "pool"?}, and every method of the app API but `wait` (it would hold a worker for up to an
// hour; poll `job` instead).
//
// `<persistence>/settings.json` ({"sequencer"?, "lending"?, "pool"?}), if present, fills in what a
// `create_wallet` or `open` request leaves out, and `status` returns it for the view's fields.
// Its "tokens" object ({"Public/<token id>": "SYMBOL"}) names tokens in the view. Dev only: its "prefill" object ({"transfer_to", "transfer_amount", "amount", "to", "index"},
// strings) fills the view's input fields at start and on Refresh.
//
// Keep public declarations on one line: logos-cpp-generator parses this header line by line.
class BosphorusModule : public LogosModuleContext {
public:
    BosphorusModule();
    ~BosphorusModule();

    BosphorusModule(const BosphorusModule&) = delete;
    BosphorusModule& operator=(const BosphorusModule&) = delete;

    std::string name() const;
    std::string version() const;

    /// {"wallet_dir", "wallet_exists", "open", "open_result", "settings"}: what the view needs to choose its first screen.
    std::string status();
    /// {"ticket": "<id>"} at once; the answer comes from take(ticket).
    std::string start(const std::string& method, const std::string& paramsJson);
    /// {"state": "pending"} or {"state": "done", "response": {"id", "result"|"error"}}; a done ticket is forgotten.
    std::string take(const std::string& ticket);
    /// Job events since the last call, without waiting: {"events": [...], "lagged": n}.
    std::string events();

private:
    std::string run(const std::string& method, const std::string& paramsJson);
    std::string walletDir();
    std::string dataDir();
    std::string settingsPath();
    static std::string takeFfiString(char* value);
    static std::string error(const std::string& code, const std::string& message);
    void reapWorkers();

    std::mutex m_mutex;
    std::map<std::string, std::string> m_done;
    std::map<std::string, std::thread> m_workers;
    std::vector<std::string> m_finished;
    std::atomic<uint64_t> m_nextTicket{1};
    std::atomic<bool> m_stopping{false};
    // Opening is serialised by m_openMutex; m_app is read by every worker.
    std::mutex m_openMutex;
    std::atomic<BosApp*> m_app{nullptr};
    std::string m_openResult;
};
