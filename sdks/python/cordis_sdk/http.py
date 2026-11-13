from .errors import CordisError


async def register_route(context, http, spec, handler):
    """ADR-0046 §3.5：路由登记与异步撤回归属同一组件效应。"""
    async def install(scope):
        number = await http.route(spec, handler)
        try:
            await scope.defer(lambda _: http.unroute(number))
        except Exception:
            await http.unroute(number)
            raise

    effect = await context.effect(install, "HTTP route", asynchronous=True)
    await effect.settled()
    return effect


async def page_attributes(metadata, page_service):
    """ADR-0046 §3.9：按核心的页面来源查询当前依赖域内的 HTTP 属性。"""
    source = metadata.get("cordis.page")
    if source is None:
        return None
    if not isinstance(source, dict) or "key" not in source:
        raise CordisError(2, "页面来源需要页面键")
    return await page_service.attributes(bytes(source["key"]))
