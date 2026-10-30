#include "bosphorus_module.h"

#include <filesystem>
#include <fstream>

#include <nlohmann/json.hpp>

using json = nlohmann::json;

BosphorusModule::BosphorusModule() = default;

BosphorusModule::~BosphorusModule()
{
    m_stopping = true;
    // No FFI call may be in flight when the app closes, so every worker is joined first. Stopping
    // the app makes calls in flight answer at once (a transfer being proven sends nothing), so
    // the joins don't wait for a proof; a job stopped mid-proof sent nothing and shows as
    // interrupted at the next open.
    if (BosApp* app = m_app.load())
        bos_app_stop(app);
    std::map<std::string, std::thread> workers;
    {
        std::lock_guard<std::mutex> lock(m_mutex);
        workers.swap(m_workers);
    }
    for (auto& [ticket, worker] : workers) {
        if (worker.joinable())
            worker.join();
    }
    if (BosApp* app = m_app.exchange(nullptr))
        bos_app_close(app);
}

std::string BosphorusModule::name() const { return "bosphorus"; }

std::string BosphorusModule::version() const { return "0.1.0"; }

std::string BosphorusModule::takeFfiString(char* value)
{
    if (!value)
        return error("internal_error", "Bosphorus returned no answer");
    std::string copy(value);
    bos_app_string_free(value);
    return copy;
}

std::string BosphorusModule::error(const std::string& code, const std::string& message)
{
    return json{{"id", nullptr}, {"error", {{"code", code}, {"message", message}}}}.dump();
}

std::string BosphorusModule::walletDir()
{
    if (!isContextReady() || instancePersistencePath().empty())
        return {};
    return (std::filesystem::path(instancePersistencePath()) / "wallet").string();
}

std::string BosphorusModule::dataDir()
{
    return (std::filesystem::path(instancePersistencePath()) / "app").string();
}

std::string BosphorusModule::settingsPath()
{
    if (!isContextReady() || instancePersistencePath().empty())
        return {};
    return (std::filesystem::path(instancePersistencePath()) / "settings.json").string();
}

// The string fields of settings.json the module knows; {} if there is no readable file.
static json readSettings(const std::string& path)
{
    json out = json::object();
    if (path.empty())
        return out;
    std::ifstream file(path);
    if (!file)
        return out;
    json s = json::parse(file, nullptr, false);
    if (!s.is_object())
        return out;
    for (const char* key : {"sequencer", "lending", "pool"}) {
        if (s.contains(key) && s[key].is_string())
            out[key] = s[key];
    }
    // "tokens": {"Public/<token id>": "SYMBOL"}, the names the view shows (token programs have
    // none). Dev only, "prefill": text for the view's input fields, so a test can drive it with
    // clicks alone.
    for (const char* key : {"tokens", "prefill"}) {
        if (!s.contains(key) || !s[key].is_object())
            continue;
        json strings = json::object();
        for (auto& [name, value] : s[key].items()) {
            if (value.is_string())
                strings[name] = value;
        }
        out[key] = strings;
    }
    return out;
}

std::string BosphorusModule::status()
{
    const std::string wallet = walletDir();
    std::error_code ignored;
    std::lock_guard<std::mutex> lock(m_openMutex);
    return json{
        {"wallet_dir", wallet.empty() ? json(nullptr) : json(wallet)},
        {"wallet_exists", !wallet.empty() && std::filesystem::exists(std::filesystem::path(wallet) / "storage.json", ignored)},
        {"open", m_app.load() != nullptr},
        {"open_result", m_openResult.empty() ? json(nullptr) : json::parse(m_openResult, nullptr, false)},
        {"settings", readSettings(settingsPath())},
    }.dump();
}

std::string BosphorusModule::start(const std::string& method, const std::string& paramsJson)
{
    if (m_stopping)
        return error("stopping", "Bosphorus is shutting down");
    if (method == "wait")
        return error("bad_request", "wait would hold a worker for up to an hour: poll job instead");
    reapWorkers();
    const std::string ticket = "t" + std::to_string(m_nextTicket++);
    std::lock_guard<std::mutex> lock(m_mutex);
    // The handle is stored before the worker can finish: it takes m_mutex to report.
    m_workers.emplace(ticket, std::thread([this, ticket, method, paramsJson]() {
        std::string response;
        try {
            response = run(method, paramsJson);
        } catch (const std::exception& e) {
            response = error("internal_error", e.what());
        }
        std::lock_guard<std::mutex> done(m_mutex);
        m_done[ticket] = response;
        m_finished.push_back(ticket);
    }));
    return json{{"ticket", ticket}}.dump();
}

std::string BosphorusModule::take(const std::string& ticket)
{
    std::lock_guard<std::mutex> lock(m_mutex);
    auto it = m_done.find(ticket);
    if (it == m_done.end()) {
        if (!m_workers.count(ticket))
            return json{{"state", "unknown"}}.dump();
        return json{{"state", "pending"}}.dump();
    }
    json out = {{"state", "done"}, {"response", json::parse(it->second, nullptr, false)}};
    m_done.erase(it);
    return out.dump();
}

std::string BosphorusModule::events()
{
    BosApp* app = m_app.load();
    if (!app)
        return json{{"events", json::array()}, {"lagged", 0}}.dump();
    json r = json::parse(takeFfiString(bos_app_poll_events(app, 0)), nullptr, false);
    if (r.is_object() && r.contains("result"))
        return r["result"].dump();
    return json{{"events", json::array()}, {"lagged", 0}}.dump();
}

void BosphorusModule::reapWorkers()
{
    std::vector<std::thread> finished;
    {
        std::lock_guard<std::mutex> lock(m_mutex);
        for (const std::string& ticket : m_finished) {
            auto it = m_workers.find(ticket);
            if (it != m_workers.end()) {
                finished.push_back(std::move(it->second));
                m_workers.erase(it);
            }
        }
        m_finished.clear();
    }
    for (std::thread& worker : finished) {
        if (worker.joinable())
            worker.join();
    }
}

std::string BosphorusModule::run(const std::string& method, const std::string& paramsJson)
{
    json params = paramsJson.empty() ? json::object() : json::parse(paramsJson, nullptr, false);
    if (params.is_discarded() || !params.is_object())
        return error("bad_request", "params must be a JSON object");
    const std::string wallet = walletDir();
    if (wallet.empty())
        return error("no_persistence", "the host gave this module no persistence directory");

    const json defaults = readSettings(settingsPath());
    if (method == "create_wallet") {
        if (!params.contains("sequencer") && defaults.contains("sequencer"))
            params["sequencer"] = defaults["sequencer"];
        params["wallet_home"] = wallet;
        return takeFfiString(bos_wallet_create(params.dump().c_str()));
    }
    if (method == "open") {
        std::lock_guard<std::mutex> lock(m_openMutex);
        if (m_app.load())
            return json{{"id", nullptr}, {"result", json::parse(m_openResult, nullptr, false)}}.dump();
        for (const char* key : {"lending", "pool"}) {
            if (!params.contains(key) && defaults.contains(key))
                params[key] = defaults[key];
        }
        params["wallet_home"] = wallet;
        params["data_dir"] = dataDir();
        BosAppOpenOutput opened = bos_app_open(params.dump().c_str());
        std::string result = takeFfiString(opened.result_json);
        if (opened.app) {
            json r = json::parse(result, nullptr, false);
            m_openResult = r.is_object() && r.contains("result") ? r["result"].dump() : "{}";
            m_app = opened.app;
        }
        return result;
    }

    BosApp* app = m_app.load();
    if (!app)
        return error("not_open", "open the wallet first");
    const json request = {{"id", nullptr}, {"method", method}, {"params", params}};
    return takeFfiString(bos_app_call(app, request.dump().c_str()));
}
