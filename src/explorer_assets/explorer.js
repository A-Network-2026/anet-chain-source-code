    const countdownNode = document.getElementById('countdown');
    let remaining = Number(countdownNode?.dataset.seconds || 0);
    const ANTS_PER_ANET = 100000000;
    const tick = () => {
        const hours = String(Math.floor(remaining / 3600)).padStart(2, '0');
        const minutes = String(Math.floor((remaining % 3600) / 60)).padStart(2, '0');
        const seconds = String(remaining % 60).padStart(2, '0');
        if (countdownNode) countdownNode.textContent = `${hours}:${minutes}:${seconds}`;
        if (remaining > 0) remaining -= 1;
    };
    tick();
    setInterval(tick, 1000);

    const params = new URLSearchParams(window.location.search);
    const fromInput = document.getElementById('tx-from');
    const toInput = document.getElementById('tx-to');
    const amountInput = document.getElementById('tx-amount');
    const feeInput = document.getElementById('tx-fee');
    // Default minimum fee: 1000 ANTS = 0.00001 ANET
    const MIN_FEE_ANET = 0.00001;
    const memoInput = document.getElementById('tx-memo');
    const seedInput = document.getElementById('tx-seed');
    const formatAnetInput = (ants) => {
        const safeAnts = Number(ants || 0);
        const whole = Math.floor(safeAnts / ANTS_PER_ANET);
        const fraction = String(safeAnts % ANTS_PER_ANET).padStart(8, '0').replace(/0+$/, '');
        return fraction ? `${whole}.${fraction}` : `${whole}`;
    };
    const parseAnetToAnts = (value, fieldLabel) => {
        const trimmed = String(value || '').trim();
        if (!trimmed) throw new Error(`${fieldLabel} is required`);
        if (!/^\d+(\.\d{0,8})?$/.test(trimmed)) {
            throw new Error(`${fieldLabel} must use ANET format with up to 8 decimal places`);
        }
        const [wholePart, fractionPart = ''] = trimmed.split('.');
        const whole = BigInt(wholePart || '0');
        const fraction = BigInt((fractionPart + '00000000').slice(0, 8));
        const ants = whole * BigInt(ANTS_PER_ANET) + fraction;
        if (ants > BigInt(Number.MAX_SAFE_INTEGER)) {
            throw new Error(`${fieldLabel} is too large to submit from this browser`);
        }
        return Number(ants);
    };
    if (params.get('from')) fromInput.value = params.get('from');
    if (params.get('to')) toInput.value = params.get('to');
    if (params.get('amount_anet')) amountInput.value = params.get('amount_anet');
    else if (params.get('amount_ants')) amountInput.value = formatAnetInput(params.get('amount_ants'));
    if (params.get('fee_anet')) feeInput.value = params.get('fee_anet');
    else if (params.get('fee_ants')) feeInput.value = formatAnetInput(params.get('fee_ants'));
    if (params.get('memo')) memoInput.value = params.get('memo');

    const txForm = document.getElementById('tx-form');
    const txResult = document.getElementById('tx-result');
    txForm?.addEventListener('submit', async (event) => {
        event.preventDefault();
        txResult.textContent = 'Submitting transfer to the colony mempool...';
        txResult.className = 'tx-result muted';

        try {
            const amountAnts = parseAnetToAnts(amountInput.value, 'Amount');
            const feeAnts = parseAnetToAnts(feeInput.value, 'Fee');
            if (feeAnts < MIN_FEE_ANET * ANTS_PER_ANET) {
                throw new Error(`Fee must be at least 0.00001 ANET (1,000 ANTS)`);
            }
            const response = await fetch('/transactions', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    from: fromInput.value.trim(),
                    to: toInput.value.trim(),
                    amount_ants: amountAnts,
                    fee_ants: feeAnts,
                    memo: memoInput.value.trim(),
                    sender_seed: seedInput.value.trim()
                })
            });
            const result = await response.json();
            if (!response.ok) throw new Error(result.error || 'Transaction request failed');
            txResult.innerHTML = `Queued transaction <strong>${result.transaction_id}</strong>. Submitted as <strong>${amountInput.value}</strong> ANET with <strong>${feeInput.value}</strong> ANET fee.`;
            txResult.className = 'tx-result ok';
            memoInput.value = '';
            seedInput.value = '';
        } catch (error) {
            txResult.textContent = error.message || 'Transaction request failed';
            txResult.className = 'tx-result error';
        }
    });

