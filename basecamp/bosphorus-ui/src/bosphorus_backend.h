#pragma once

#include "logos_ui_plugin_context.h"
#include "rep_bosphorus_backend_source.h"

// The view's bridge to the core module. It holds nothing: the wallet, jobs and keys live in the
// core's process, and every call is non-blocking there.
class BosphorusBackend : public BosphorusBackendSimpleSource, public LogosUiPluginContext {
public:
    QString status() override;
    QString start(QString method, QString params) override;
    QString take(QString ticket) override;
    QString events() override;
};
