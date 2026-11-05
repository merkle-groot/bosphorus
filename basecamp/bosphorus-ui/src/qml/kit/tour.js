.pragma library
// The guided tour: one step per dimmed view. A step names the tab it shows and the items it
// lights up (by objectName), each with a short callout and the side the callout sits on. `setup`
// is what the view must do first (e.g. open a market in Position); `fit` lights a container's
// contents rather than all of it. Callouts stay under ~9 words.

var steps = [
    { tab: 0, title: "Welcome aboard",
      body: "Bosphorus is a lending market with two depths. What floats is public; what's below the surface is private, and only this computer can see it.",
      targets: [{ name: "nav", label: "Four screens: switch here", side: "below" },
                { name: "statusPill", label: "Network status: green is good", side: "below" }] },
    { tab: 0, title: "Your Everyday account",
      body: "Up here works like any wallet: anyone can see these balances. Copy your address to get paid.",
      targets: [{ name: "addressChip", label: "Your address: click to copy", side: "above" },
                { name: "rowActions", label: "Move ↓ makes it private · ⋯ to send", side: "left" }] },
    { tab: 0, title: "Below the surface",
      body: "A dive moves tokens into your private account. Your computer proves it in a minute or two; the network sees something leave, not where it went.",
      targets: [{ name: "zoneBelow", label: "Private: only this computer can see it", side: "above" }] },
    { tab: 1, title: "Markets",
      body: "Each market lends one token against another. Lenders earn what borrowers pay.",
      targets: [{ name: "marketRow", label: "Borrow USDC against WETH or WBTC", side: "below" },
                { name: "marketRates", label: "What lenders earn, what borrowers pay", side: "right" }] },
    { tab: 1, title: "Before you borrow",
      body: "You can borrow up to 80% of what your collateral is worth. Prices come from an oracle; borrowing pauses while a price is old.",
      targets: [{ name: "borrowLimit", label: "Past the sell line, collateral is sold", side: "left" },
                { name: "marketPrice", label: "Live price, and how fresh it is", side: "left" },
                { name: "marketActions", label: "Start here", side: "below" }] },
    { tab: 2, setup: "position", title: "Lending and borrowing",
      body: "What you've lent and what you've borrowed, side by side. Each button opens one action: how much, then go.",
      targets: [{ name: "positionLend", label: "Lend, earn, take it back", side: "above" },
                { name: "positionBorrow", label: "Put up collateral, borrow, pay back", side: "above" }] },
    { tab: 2, setup: "position", title: "Watch the water",
      body: "The water is your loan against what you can borrow. When you borrow, a dashed line shows where it would rise before you press Go.",
      targets: [{ name: "sellLine", label: "Water here: some collateral is sold", side: "below" }] },
    { tab: 3, title: "Activity",
      body: "Every action shows up here, step by step, with its transaction. If Bosphorus closes mid-way, it picks up where it stopped.",
      targets: [{ name: "activityList", fit: true, label: "Every action, step by step", side: "right" },
                { name: "seabed", label: "All working: nothing to worry about", side: "above" }] },
    { tab: 0, title: "That's the tour",
      body: "You've borrowed and lent below the surface. From here: pay back from Position, or dive tokens up and down on this screen.",
      targets: [{ name: "zoneBelow", label: "Your private money", side: "above" },
                { name: "nav", label: "Position: pay back, add, take back", side: "below" }] }
]
