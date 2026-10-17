-- Legacy attempts have no recoverable exact input; leave their input NULL.
ALTER TABLE flag_submitter_batches ADD COLUMN input_gzip BLOB;
