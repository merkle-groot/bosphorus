#include "bosphorus_backend.h"

#include "logos_sdk.h"

QString BosphorusBackend::status() { return modules().bosphorus.status(); }

QString BosphorusBackend::start(QString method, QString params) { return modules().bosphorus.start(method, params); }

QString BosphorusBackend::take(QString ticket) { return modules().bosphorus.take(ticket); }

QString BosphorusBackend::events() { return modules().bosphorus.events(); }
