async def action(page, operation):
    panel = page.locator('[data-plugin="realm-counter"]')
    button = panel.get_by_role("button", name=operation, exact=True)
    await button.click()
    await page.wait_for_function("button => !button.disabled", arg=await button.element_handle())
    assert await panel.locator('p[role="status"]').inner_text() == "页面服务调用完成"
    return int(await panel.get_by_label("页面域计数", exact=True).inner_text())


async def run(case):
    """DOC-15 §11.2、AU-14：同名派生域保持页面隔离，关闭一页保留另一页的服务。"""
    first, second = case.pages
    await case.wait(
        lambda: all(item["state"] == "active" for item in case.status()["instances"]),
        "两页命名域组件激活",
    )
    for page in case.pages:
        launch = page.get_by_role("button", name="连接页面计数", exact=True)
        handle = await launch.element_handle()
        await launch.click()
        await page.wait_for_function("button => ['ready', 'failed'].includes(button.dataset.state)", arg=handle)
        assert await handle.get_attribute("data-state") == "ready", await handle.text_content()
        await page.get_by_label("页面域计数", exact=True).wait_for()
    owners = [int(await page.locator('[data-plugin="realm-counter"]').get_attribute("data-owner")) for page in case.pages]
    before = case.status()
    records = [next(item for item in before["instances"] if item["context"] == owner) for owner in owners]
    initial = [await action(page, "读取页面计数") for page in case.pages]
    first_value = await action(first, "增加页面计数")
    second_value = await action(second, "读取页面计数")
    observed = {"realm": "shared-browser-test", "owners": owners, "initial": initial, "after_first_increment": [first_value, second_value], "before": before}
    case.save("realm-observed.json", observed)
    for index, page in enumerate(case.pages):
        await page.screenshot(path=str(case.project / f"realm-page-{index + 1}.png"), full_page=True)
    assert initial == [0, 0], observed
    assert [first_value, second_value] == [1, 0], observed
    remaining = {item["name"]: (item["context"], item["revision"]) for item in before["instances"] if item["group"] == records[1]["group"]}
    await first.close()
    await case.wait(
        lambda: all(item["group"] != records[0]["group"] for item in case.status()["instances"]),
        "已关闭页面的子实例回收",
    )
    assert await action(second, "读取页面计数") == 0
    assert await action(second, "增加页面计数") == 1
    after = case.status()
    current = {item["name"]: (item["context"], item["revision"]) for item in after["instances"] if item["group"] == records[1]["group"]}
    assert current == remaining, {"before": remaining, "after": current}
    assert after["pid"] == before["pid"]
    observed["after_close"] = after
    observed["remaining_value"] = 1
    case.save("realm-close.json", observed)
    await second.screenshot(path=str(case.project / "realm-remaining.png"), full_page=True)
    return observed
