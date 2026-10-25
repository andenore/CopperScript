"""Conservative single-frequency conducted interference budget, in integer mdB.

Not an antenna/field solver or RF approval. Inputs must be guaranteed bounds
at the same frequency and operating condition. Do not interpolate receiver
immunity or filter rejection from isolated datasheet samples. Antenna S21 is
only one coupling path; PCB, supply and harmonic coupling need separate review.
"""


def isolation_budget(*, transmitter_max_mdbm: int, receiver_immunity_mdbm: int,
                     guard_mdb: int, isolation_min_mdb: int | None = None,
                     filter_rejection_min_mdb: int | None = None) -> dict:
    """Screen minimum isolation + rejection against a receiver input limit.

    None means unknown, not zero attenuation. An explicitly absent external
    filter can be modelled as zero rejection. A favourable screen never waives
    prototype tests, passband loss, filter power/ESD, or full band coverage.
    """
    values = dict(transmitter_max_mdbm=transmitter_max_mdbm,
                  receiver_immunity_mdbm=receiver_immunity_mdbm,
                  guard_mdb=guard_mdb, isolation_min_mdb=isolation_min_mdb,
                  filter_rejection_min_mdb=filter_rejection_min_mdb)
    for name, value in values.items():
        if value is None and name in ('isolation_min_mdb', 'filter_rejection_min_mdb'):
            continue
        if type(value) is not int:
            raise ValueError(f'{name} must be an integer bound with mdB units')
        if name in ('guard_mdb', 'isolation_min_mdb', 'filter_rejection_min_mdb') and value < 0:
            raise ValueError(f'{name} cannot be negative')
    required = max(0, transmitter_max_mdbm - receiver_immunity_mdbm + guard_mdb)
    result = dict(values, required_total_attenuation_mdb=required,
                  required_isolation_mdb=None, predicted_receiver_input_mdbm=None,
                  margin_mdb=None, status='incomplete', qualified_release=False)
    if filter_rejection_min_mdb is not None:
        result['required_isolation_mdb'] = max(0, required - filter_rejection_min_mdb)
    if isolation_min_mdb is not None and filter_rejection_min_mdb is not None:
        coupled = transmitter_max_mdbm - isolation_min_mdb - filter_rejection_min_mdb
        margin = receiver_immunity_mdbm - guard_mdb - coupled
        result.update(predicted_receiver_input_mdbm=coupled, margin_mdb=margin,
                      status='within_bound' if margin >= 0 else 'insufficient_attenuation')
    return result
